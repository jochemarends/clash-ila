use std::io::{Result as IoResult, Seek};
use std::path::PathBuf;

use clap::ValueEnum;

use crate::communication::SignalCluster;
use crate::config::IlaConfig;
use crate::vcd::{VcdWriter, VcdModule};

/// Controls how [`SignalCluster`]s are exported.
#[derive(Debug, Copy, Clone, PartialEq, ValueEnum)]
pub enum AutoExportMode {
    /// Before writing each [`SignalCluster`], truncates the file, then writes the VCD header,
    /// variable definition section, and variable initialization section.
    #[value(help = "Overwrite the file with the last captured samples")]
    Truncate,
    /// Writes the VCD header, variable definition section, and variable initialization section
    /// before writing the first [`SignalCluster`]. Subsequent [`SignalCluster`]s are appended to
    /// the data dump section.
    #[value(help = "Append the last captured samples to the file")]
    Append,
}

impl std::fmt::Display for AutoExportMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", match self {
            AutoExportMode::Truncate => "TRUNCATE",
            AutoExportMode::Append => "APPEND",
        })
    }
}

impl TryFrom<u32> for AutoExportMode {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(AutoExportMode::Truncate),
            1 => Ok(AutoExportMode::Append),
            _ => Err(()),
        }
    }
}

/// Configuration for an auto-export session.
pub struct AutoExportConfig {
    /// Path of the file to export to.
    pub path: PathBuf,
    /// Export mode to use.
    pub mode: AutoExportMode,
}

// Represents an auto-export session.
pub struct AutoExportSession {
    config: AutoExportConfig,
    writer: VcdWriter<std::io::BufWriter<std::fs::File>>,
    preamble_written: bool,
    started_at: std::time::Instant,
}

impl AutoExportSession {
    /// Creates a new auto-export session starting at the current time.
    ///
    /// # Errors
    ///
    /// Returns an error if the export file could not be opened or created.
    pub fn new(config: AutoExportConfig, ila: &IlaConfig) -> IoResult<Self> {
        let file = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&config.path)?;

        let writer = std::io::BufWriter::new(file);

        let mut root = VcdModule::new(ila.toplevel.clone());
        for signal in ila.inputs.iter() {
            root = root.add_wire(signal.name.clone(), signal.width);
        }
        let vcd_writer = root.writer(writer);

        Ok(Self {
            config,
            writer: vcd_writer,
            preamble_written: false,
            started_at: std::time::Instant::now(),
        })
    }

    /// Exports a [`SignalCluster`] as part of an auto-export session.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to or flushing the file fails.
    pub fn export_cluster(&mut self, signals: &SignalCluster) -> IoResult<()> {
        let (should_truncate_file, should_write_preamble) = match self.config.mode {
            AutoExportMode::Truncate => (true, true),
            AutoExportMode::Append => (false, !self.preamble_written),
        };

        if should_truncate_file {
            let file = self.writer.writer_mut().get_mut();
            file.set_len(0)?;
            file.rewind()?;
        }

        if should_write_preamble {
            self.writer.write_preamble()?;
            self.preamble_written = true;
        }

        self.writer.write_cluster(signals)?;
        self.writer.flush()
    }

    /// Get the configuration this session was created with.
    pub fn config(&self) -> &AutoExportConfig {
        &self.config
    }

    /// Get the instant when this session was started.
    pub fn started_at(&self) -> &std::time::Instant {
        &self.started_at
    }
}
