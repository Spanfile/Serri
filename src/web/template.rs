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
#[template(path = "device/device.html")]
pub struct DeviceTemplate {
    pub index_template: IndexTemplate,
    pub preserve_history: bool,
}

#[derive(Template)]
#[template(path = "device/device_popout.html")]
pub struct DevicePopoutTemplate(pub DeviceTemplate);

#[derive(Template)]
#[template(path = "about.html")]
pub struct AboutTemplate {
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

impl Deref for DevicePopoutTemplate {
    type Target = DeviceTemplate;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Deref for AboutTemplate {
    type Target = BaseTemplate;

    fn deref(&self) -> &Self::Target {
        &self.base_template
    }
}
