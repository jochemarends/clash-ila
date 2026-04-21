use std::{io::Result as IoResult, io::Write as IoWrite, path::Path};
use vcd::{IdCode, SimulationCommand};

use crate::{communication::SignalCluster, config::IlaConfig};

/// Write the measured data to a VCD file
///
/// * `signals` - The signal cluster to write to a VCD file
/// * `identifier` - The collective name of the signals
/// * `path` - Path to the write to write too, will overwrite any file already in place
pub fn write_to_vcd<P: AsRef<Path>>(
    signals: &SignalCluster,
    config: &IlaConfig,
    path: P,
) -> IoResult<()> {
    let file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;

    let writer = std::io::BufWriter::new(file);

    let mut vcd_config = VcdWriterConfig::with_module(config.toplevel.clone());

    for signal in &signals.cluster {
        vcd_config.add_wire(signal.name.to_string(), signal.width);
    }

    let mut vcd_writer = vcd_config.writer(writer);
    vcd_writer.write_preamble()?;
    vcd_writer.write_cluster(&signals)?;
    vcd_writer.flush()
}

fn unknown_bits(width: usize) -> impl Iterator<Item = vcd::Value> {
    std::iter::repeat(vcd::Value::X).take(width)
}

/// Configuration builder for [`VcdWriter`].
///
/// Specifies the module and variables that a [`VcdWriter`] will emit.
///
/// # Examples
///
/// ```
/// use std::fs::File;
/// use crate::vcd::VcdWriterConfig;
///
/// let file = File::create("dump.vcd").unwrap();
///
/// let mut vcd_writer = VcdWriterConfig::with_module("mux")
///     .add_wire("a", 8)
///     .add_wire("b", 8)
///     .add_wire("select", 1)
///     .add_wire("output", 8)
///     .writer(file);
/// ```
pub struct VcdWriterConfig {
    vars: Vec<vcd::Var>,
    module: String,
}

impl VcdWriterConfig {
    /// Constructs a new [`VcdWriterConfig`] with the specified module.
    pub fn with_module(module: String) -> Self {
        Self {
            vars: Vec::new(),
            module: module,
        }
    }

    /// Adds a wire with the given name and width.
    ///
    /// The wire will be added to the module associated with this configuration.
    pub fn add_wire(&mut self, name: String, width: usize) -> &mut Self {
        let var_id = self.next_var_id();
        let var = vcd::Var::new(vcd::VarType::Wire, width as u32, var_id, name, None);
        self.vars.push(var);
        self
    }

    /// Constructs a new [`VcdWriter`] with this configuration.
    pub fn writer<W: IoWrite>(self, writer: W) -> VcdWriter<W> {
        VcdWriter::new(writer, self)
    }

    fn next_var_id(&self) -> IdCode {
        if let Some(wire) = self.vars.last() {
            wire.code.next()
        } else {
            IdCode::FIRST
        }
    }
}

/// Wraps a [`std::io::Write`] for incrementally writing [`SignalCluster`]s as VCD.
pub struct VcdWriter<W: IoWrite> {
    inner: vcd::Writer<W>,
    config: VcdWriterConfig,
    time: u64,
}

impl<W: IoWrite> VcdWriter<W> {
    /// Construct a new VCD writer with the given writer and configuration.
    fn new(writer: W, config: VcdWriterConfig) -> Self {
        Self {
            inner: vcd::Writer::new(writer),
            config,
            time: 0,
        }
    }

    /// Writes everything that should precede the data dump section of a VCD.
    ///
    /// # Errors
    ///
    /// Returns an error if writing to the underlying writer fails.
    pub fn write_preamble(&mut self) -> IoResult<()> {
        // Header
        self.inner.timescale(1, vcd::TimescaleUnit::US)?;
        self.inner.add_module(&self.config.module)?;
        for var in &self.config.vars {
            self.inner.var(var)?;
        }
        self.inner.upscope()?;
        self.inner.enddefinitions()?;

        // Initialize all variables to unknown
        self.inner.begin(SimulationCommand::Dumpvars)?;
        for wire in &self.config.vars {
            self.inner.change_vector(wire.code, unknown_bits(wire.size as usize))?;
        }
        self.inner.end()?;

        Ok(())
    }

    /// Writes a [`SignalCluster`] as VCD.
    ///
    /// Signals are mapped to variables by their index: the Nth signal in the cluster is written to
    /// the Nth variable specified the configuration.
    ///
    /// The timestamp is advanced by the largest number of samples of any signal in the cluster.
    ///
    /// Signals with fewer samples are padded with undefined values.
    ///
    /// # Errors
    ///
    /// Propagates errors from the underlying writer, or returns an error if the provided
    /// [`SignalCluster`] contains a different number of signals than the number of variables
    /// specified in this writer's configuration.
    pub fn write_cluster(&mut self, signals: &SignalCluster) -> IoResult<()> {
        if signals.cluster.len() != self.config.vars.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "VCD write error in module '{}': expected {} signals, got {}",
                    self.config.module,
                    self.config.vars.len(),
                    signals.cluster.len(),
                ),
            ));
        }

        let max_sample_count = signals
            .cluster
            .iter()
            .map(|b| b.samples.len())
            .max()
            .ok_or(std::io::ErrorKind::InvalidData)?;

        for index in 0..max_sample_count {
            let t = self.time + (index as u64);
            self.inner.timestamp(t)?;

            for (wire, signal) in self.config.vars.iter().zip(&signals.cluster) {
                if let Some(sample) = signal.samples.get(index) {
                    let current_vector = sample.iter()
                        .map(|b| b.then_some(vcd::Value::V1).unwrap_or(vcd::Value::V0));

                    self.inner.change_vector(wire.code, current_vector)?;
                } else {
                    self.inner.change_vector(wire.code, unknown_bits(wire.size as usize))?;
                }
            }
        }

        self.time += max_sample_count as u64;
        self.inner.timestamp(self.time)?;

        Ok(())
    }

    /// Flushes the underlying writer.
    ///
    /// # Errors
    ///
    /// Returns an error if the underlying writer fails to flush.
    pub fn flush(&mut self) -> IoResult<()> {
        self.inner.flush()
    }

    /// Get a mutable reference to the underlying writer.
    pub fn writer_mut(&mut self) -> &mut W {
        self.inner.writer()
    }
}
