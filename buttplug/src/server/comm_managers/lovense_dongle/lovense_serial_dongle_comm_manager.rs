use super::{
  lovense_dongle_messages::{
    LovenseDeviceCommand,
    LovenseDongleIncomingMessage,
    OutgoingLovenseData,
  },
  lovense_dongle_state_machine::create_lovense_dongle_machine,
};
use crate::{
  core::ButtplugResultFuture,
  server::comm_managers::{
    DeviceCommunicationEvent,
    DeviceCommunicationManager,
    DeviceCommunicationManagerCreator,
  },
  util::async_manager,
};
use async_channel::{bounded, Receiver, Sender};
use async_mutex::Mutex;
use futures::StreamExt;
use serde_json::Deserializer;
use serialport::{
  available_ports,
  open_with_settings,
  SerialPort,
  SerialPortSettings,
  SerialPortType,
};
use std::{
  io::ErrorKind,
  sync::{atomic::AtomicBool, Arc},
  thread,
  time::Duration,
};
use tracing_futures::Instrument;

fn serial_write_thread(mut port: Box<dyn SerialPort>, mut receiver: Receiver<OutgoingLovenseData>) {
  let mut port_write = |mut data: String| {
    data += "\r\n";
    info!("Writing message: {}", data);

    // TODO WRITE SHOULD ALWAYS BE FOLLOWED BY A READ UNLESS "EAGER" IS USED
    //
    // We should check this on the outgoing message. Otherwise we will run into
    // all sorts of trouble.
    port.write_all(&data.into_bytes()).unwrap();
  };
  while let Some(data) = async_manager::block_on(async { receiver.next().await }) {
    match data {
      OutgoingLovenseData::Raw(s) => {
        port_write(s);
      }
      OutgoingLovenseData::Message(m) => {
        port_write(serde_json::to_string(&m).unwrap());
      }
    }
  }
  info!("EXITING LOVENSE DONGLE WRITE THREAD.");
}

fn serial_read_thread(mut port: Box<dyn SerialPort>, sender: Sender<LovenseDongleIncomingMessage>) {
  let mut data: String = String::default();
  loop {
    let mut buf: [u8; 1024] = [0; 1024];
    match port.read(&mut buf) {
      Ok(len) => {
        debug!("Got {} serial bytes", len);
        data += std::str::from_utf8(&buf[0..len]).unwrap();
        if data.contains('\n') {
          debug!("Serial Buffer: {}", data);

          let sender_clone = sender.clone();
          let stream =
            Deserializer::from_str(&data).into_iter::<LovenseDongleIncomingMessage>();
          for msg in stream {
            match msg {
              Ok(m) => {
                debug!("Read message: {:?}", m);
                async_manager::block_on(async { sender_clone.send(m).await.unwrap() });
              }
              Err(e) => {
                error!("Error reading: {:?}", e);
                /*
                sender_clone
                  .send(IncomingLovenseData::Raw(incoming.clone().to_string()))
                  .await;
                  */
              }
            }
          }

          // TODO We don't seem to have an extra coming through at the moment,
          // but might need this later?
          data = String::default();
        }
      }
      Err(e) => {
        if e.kind() == ErrorKind::TimedOut {
          continue;
        }
        error!("{:?}", e);
        break;
      }
    }
  }
  info!("EXITING LOVENSE DONGLE READ THREAD.");
}
pub struct LovenseSerialDongleCommunicationManager {
  machine_sender: Sender<LovenseDeviceCommand>,
  //port: Arc<Mutex<Option<Box<dyn SerialPort>>>>,
  read_thread: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
  write_thread: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}

impl LovenseSerialDongleCommunicationManager {
  fn find_dongle(&self) -> ButtplugResultFuture {
    // First off, see if we can actually find a Lovense dongle. If we already
    // have one, skip on to scanning. If we can't find one, send message to log
    // and stop scanning.

    let machine_sender_clone = self.machine_sender.clone();
    let held_read_thread = self.read_thread.clone();
    let held_write_thread = self.write_thread.clone();
    Box::pin(async move {
      // TODO Does this block? Should it run in one of our threads?
      match available_ports() {
        Ok(ports) => {
          info!("Got {} serial ports back", ports.len());
          for p in ports {
            if let SerialPortType::UsbPort(usb_info) = p.port_type {
              // Hardcode the dongle VID/PID for now. We can't really do protocol
              // detection here because this is a comm bus to us, not a device.
              if usb_info.vid == 0x1a86 && usb_info.pid == 0x7523 {
                // We've found a dongle.
                info!("Found lovense dongle, connecting");
                let mut settings = SerialPortSettings::default();
                // Default is 8/N/1 but we'll need to set the baud rate
                settings.baud_rate = 115200;
                // Set our timeout at ~2hz. Would be nice if this was async, but oh well.
                settings.timeout = Duration::from_millis(500);
                match open_with_settings(&p.port_name, &settings) {
                  Ok(dongle_port) => {
                    let (writer_sender, writer_receiver) = bounded(256);
                    let (reader_sender, reader_receiver) = bounded(256);

                    let read_port = (*dongle_port).try_clone().unwrap();
                    let read_thread = thread::Builder::new()
                      .name("Serial Reader Thread".to_string())
                      .spawn(move || {
                        serial_read_thread(read_port, reader_sender);
                      })
                      .unwrap();

                    let write_port = (*dongle_port).try_clone().unwrap();
                    let write_thread = thread::Builder::new()
                      .name("Serial Writer Thread".to_string())
                      .spawn(move || {
                        serial_write_thread(write_port, writer_receiver);
                      })
                      .unwrap();

                    *(held_read_thread.lock().await) = Some(read_thread);
                    *(held_write_thread.lock().await) = Some(write_thread);
                    machine_sender_clone
                      .send(LovenseDeviceCommand::DongleFound(
                        writer_sender,
                        reader_receiver,
                      ))
                      .await
                      .unwrap();
                  }
                  Err(e) => error!("{:?}", e),
                };
              }
            }
          }
        }
        Err(_) => {
          info!("No serial ports found");
        }
      }
      Ok(())
    })
  }
}

impl DeviceCommunicationManagerCreator for LovenseSerialDongleCommunicationManager {
  fn new(event_sender: Sender<DeviceCommunicationEvent>) -> Self {
    info!("Lovense dongle serial port created!");
    let (machine_sender, machine_receiver) = bounded(256);
    let mgr = Self {
      machine_sender,
      read_thread: Arc::new(Mutex::new(None)),
      write_thread: Arc::new(Mutex::new(None)),
    };
    let dongle_fut = mgr.find_dongle();
    // TODO If we don't find a dongle before scanning, what happens?
    async_manager::spawn(async move {
      if let Err(err) = dongle_fut.await {
        error!("Error finding dongle: {:?}", err);
      }
    })
    .unwrap();
    async_manager::spawn(
      async move {
        let (mut machine, _) = create_lovense_dongle_machine(event_sender, machine_receiver);
        while let Some(next) = machine.transition().await {
          machine = next;
        }
      }
      .instrument(tracing::info_span!("Lovense Dongle State Machine")),
    )
    .unwrap();
    mgr
  }
}

impl DeviceCommunicationManager for LovenseSerialDongleCommunicationManager {
  fn name(&self) -> &'static str {
    "LovenseDongleCommunicationManager"
  }

  fn start_scanning(&self) -> ButtplugResultFuture {
    info!("Lovense Dongle Manager scanning ports!");
    let sender = self.machine_sender.clone();
    Box::pin(async move {
      sender
        .send(LovenseDeviceCommand::StartScanning)
        .await
        .unwrap();
      Ok(())
    })
  }

  fn stop_scanning(&self) -> ButtplugResultFuture {
    let sender = self.machine_sender.clone();
    Box::pin(async move {
      sender
        .send(LovenseDeviceCommand::StopScanning)
        .await
        .unwrap();
      Ok(())
    })
  }

  fn scanning_status(&self) -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
  }
}
