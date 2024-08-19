#![feature(let_chains)]

use std::fs::read_to_string;

use crate::config::SerriConfig;

mod config;
mod util;
mod web;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let config: SerriConfig = toml::from_str(&read_to_string("serri.toml")?)?;
    println!("{config:#?}");

    web::run(config).await?;
    Ok(())
}
