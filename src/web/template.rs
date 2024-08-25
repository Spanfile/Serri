use std::{ops::Deref, sync::Arc};

use askama_axum::Template;

use crate::config::SerriConfig;

#[derive(Template)]
#[template(path = "base.html")]
pub struct BaseTemplate {
    pub serri_config: Arc<SerriConfig>,
    pub active_path: &'static str,
}

#[derive(Template)]
#[template(path = "404.html")]
pub struct NotFoundTemplate {
    pub base_template: BaseTemplate,
}

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate {
    pub base_template: BaseTemplate,
    pub active_device_index: Option<usize>,
}

#[derive(Template)]
#[template(path = "device.html")]
pub struct DeviceTemplate {
    pub index_template: IndexTemplate,
    pub preserve_history: bool,
}

#[derive(Template)]
#[template(path = "config.html")]
pub struct ConfigTemplate {
    pub base_template: BaseTemplate,
}

impl Deref for NotFoundTemplate {
    type Target = BaseTemplate;

    fn deref(&self) -> &Self::Target {
        &self.base_template
    }
}

impl Deref for IndexTemplate {
    type Target = BaseTemplate;

    fn deref(&self) -> &Self::Target {
        &self.base_template
    }
}

impl Deref for DeviceTemplate {
    type Target = IndexTemplate;

    fn deref(&self) -> &Self::Target {
        &self.index_template
    }
}

impl Deref for ConfigTemplate {
    type Target = BaseTemplate;

    fn deref(&self) -> &Self::Target {
        &self.base_template
    }
}
