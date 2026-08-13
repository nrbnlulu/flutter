// Copyright 2026 The Flutter Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

mod frb_generated;

pub mod api;

use flutter_plugin_sdk::{PluginError, PluginRegistrar};

pub fn register_application(registrar: &mut PluginRegistrar) -> Result<(), PluginError> {
    api::register(registrar)
}
