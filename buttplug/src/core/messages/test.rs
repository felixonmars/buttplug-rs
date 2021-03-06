// Buttplug Rust Source Code File - See https://buttplug.io for more info.
//
// Copyright 2016-2020 Nonpolynomial Labs LLC. All rights reserved.
//
// Licensed under the BSD 3-Clause license. See LICENSE file in the project root
// for full license information.

use super::*;
#[cfg(feature = "serialize-json")]
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, ButtplugMessage, Clone, PartialEq)]
#[cfg_attr(feature = "serialize-json", derive(Serialize, Deserialize))]
pub struct Test {
  /// Message Id, used for matching message pairs in remote connection instances.
  #[cfg_attr(feature = "serialize-json", serde(rename = "Id"))]
  pub(super) id: u32,
  /// Test string, which will be echoed back to client when sent to server.
  #[cfg_attr(feature = "serialize-json", serde(rename = "TestString"))]
  pub test_string: String,
}

impl Test {
  /// Creates a new Test message with the given Id.
  pub fn new(test: &str) -> Self {
    Self {
      id: 1,
      test_string: test.to_owned(),
    }
  }
}
