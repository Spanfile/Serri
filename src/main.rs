#![feature(ascii_char)]

use std::fs::read_to_string;

use crate::config::SerriConfig;

mod config;
mod util;

fn main() -> anyhow::Result<()> {
    let config: SerriConfig = toml::from_str(&read_to_string("serri.toml")?)?;
    println!("{config:#?}");

    Ok(())
}
