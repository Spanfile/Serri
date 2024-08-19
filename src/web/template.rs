use std::{ops::Deref, sync::Arc};

use askama_axum::Template;

use crate::config::SerriConfig;

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate {
    pub config: Arc<SerriConfig>,
    pub active_device_index: Option<usize>,
    pub active_path: String,
}

#[derive(Template)]
#[template(path = "device.html")]
pub struct DeviceTemplate {
    pub index_template: IndexTemplate,
}

impl Deref for DeviceTemplate {
    type Target = IndexTemplate;

    fn deref(&self) -> &Self::Target {
        &self.index_template
    }
}
