use std::{
    path::{Path, PathBuf},
};
use clap::Args;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IlaSignal {
    pub name: String,
    pub width: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IlaTrigger {
    /// Name of the trigger which is displayed in the TUI.
    pub name: String,
    /// Unique identifier of trigger used in Lua config.
    pub id: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct IlaConfig {
    pub toplevel: String,
    #[serde(rename = "bufferSize")]
    pub buffer_size: usize,
    pub hash: u32,
    pub signals: Vec<IlaSignal>,
    #[serde(rename = "triggerNames")]
    pub trigger_names: Vec<String>,
    #[serde(rename = "triggerIds")]
    pub trigger_ids: Vec<String>,
}

impl IlaConfig {
    /// How many bits does it take to get one sample of all signals?
    #[inline]
    pub fn transaction_bit_count(&self) -> usize {
        self.signals.iter().map(|signal| signal.width).sum()
    }

    /// How many bytes does it take to get one sample of all signals?
    #[inline]
    #[allow(unused)]
    pub fn transaction_byte_count(&self) -> usize {
        self.transaction_bit_count().div_ceil(8)
    }

    /// How many bytes do we expect a datapacket to contain?
    #[inline]
    #[allow(unused)]
    pub fn expected_byte_count(&self) -> usize {
        self.buffer_size * self.transaction_byte_count()
    }
}

#[derive(Args, Debug)]
#[group(required = true, multiple = false)]
pub struct ConfigMethod {
    #[arg(short, long, help = "The path leading to the ILA config file")]
    file: Option<PathBuf>,

    #[arg(
        short,
        long,
        help = "The contents of the ILA config file directly given as an argument"
    )]
    content: Option<String>,
}

impl ConfigMethod {
    pub fn get_config(&self) -> std::io::Result<IlaConfig> {
        if let Some(file) = &self.file {
            return read_config(file);
        }

        if let Some(content) = &self.content {
            return read_config_directly(content);
        }

        Err(std::io::ErrorKind::Unsupported.into())
    }
}

pub fn read_config_directly(content: &str) -> std::io::Result<IlaConfig> {
    Ok(serde_json::from_str(&content)?)
}

pub fn read_config<P>(path: P) -> std::io::Result<IlaConfig>
where
    P: AsRef<Path>,
{
    let json_content = std::fs::read_to_string(path)?;

    Ok(serde_json::from_str(&json_content)?)
}
