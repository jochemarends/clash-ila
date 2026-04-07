use std::{io::Result as IoResult, io::Write as IoWrite, path::Path};
use vcd::{IdCode, SimulationCommand, Value as VcdValue};

use crate::{communication::{Signal, SignalCluster}, config::IlaConfig};

// type VcdWriter = vcd::Writer<std::io::BufWriter<std::fs::File>>;
type VcdBitVec = Vec<VcdValue>;

/// An internal structure used to better represent signals for the VCD writer
struct VcdSignal {
    /// Wire definition, used as a handle to write to the file
    wire: IdCode,
    /// A bit vector consisting of unknowns, used in the wire definition and at the end of the
    /// signal if the signal is shorter than others
    unknown: VcdBitVec,
    /// The actual measured values
    data: Vec<VcdBitVec>,
}

impl VcdSignal {
    /// Convert between a regular DataPacket into a VcdSignal
    ///
    /// Sadly this function cannot be implemented using `Into` because it will directly write the
    /// signal definition to the file.
    fn from_vcd<W: IoWrite>(vcd_writer: &mut vcd::Writer<W>, signal: &Signal) -> IoResult<VcdSignal> {
        let wire =
            vcd_writer.add_wire(signal.width as u32, &signal.name)?;

        Ok(VcdSignal {
            wire,
            unknown: vec![VcdValue::X; signal.width],
            data: signal
                .samples
                .iter()
                .map(|v| {
                    v.iter()
                        .map(|b| b.then_some(VcdValue::V1).unwrap_or(VcdValue::V0))
                        .collect()
                })
                .collect(),
        })
    }
}

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
    let mut vcd_writer = vcd::Writer::new(std::io::BufWriter::new(file));

    // Header
    vcd_writer.timescale(1, vcd::TimescaleUnit::US)?;
    vcd_writer.add_module(&config.toplevel)?;
    let wires: Vec<VcdSignal> = signals
        .cluster
        .iter()
        .filter_map(|signal| VcdSignal::from_vcd(&mut vcd_writer, signal).ok())
        .collect();
    vcd_writer.upscope()?;
    vcd_writer.enddefinitions()?;

    // Initialize all variables to unknown
    vcd_writer.begin(SimulationCommand::Dumpvars)?;
    for wire in &wires {
        vcd_writer.change_vector(wire.wire, wire.unknown.clone())?;
    }
    vcd_writer.end()?;

    // Actual change value part
    let max_sample_count = signals
        .cluster
        .iter()
        .map(|b| b.samples.len())
        .max()
        .ok_or(std::io::ErrorKind::InvalidData)?;

    for t in 0..max_sample_count {
        vcd_writer.timestamp(t as u64)?;

        for signal in &wires {
            let current_vector = signal.data.get(t).unwrap_or(&signal.unknown).to_owned();
            vcd_writer.change_vector(signal.wire, current_vector)?;
        }
    }
    vcd_writer.timestamp(max_sample_count as u64)?;

    // Ensure any unwritten data is flushed
    vcd_writer.flush()?;
    Ok(())
}

/// Contains everything needed to define a VCD variable of type "wire"
#[derive(Clone)]
struct VcdWire {
    width: usize,
    name: String,
    id: IdCode,
}

impl VcdWire {
    /// A bit vector consisting of unknowns, used in the wire definition and at the end of the
    /// signal if the signal is shorter than others
    fn unknown(&self) -> impl Iterator<Item = VcdValue> {
        std::iter::repeat(VcdValue::X).take(self.width)
    }
}

/// Configuration for a [`VcdWriter`]
///
/// This builder can be used to describe VCD variables the name of the module they are associated
/// with.
pub struct VcdWriterConfig {
    wires: Vec<VcdWire>,
    module: String,
}

impl VcdWriterConfig {
    pub fn with_module(module: impl Into<String>) -> Self {
        Self {
            wires: Vec::new(),
            module: module.into(),
        }
    }

    pub fn add_wire(&mut self, name: &str, width: usize) -> &mut Self {
        self.wires.push(VcdWire {
            name: name.to_owned(),
            width: width,
            id: self.next_wire_id(),
        });

        self
    }

    pub fn writer<W: IoWrite>(self, writer: W) -> VcdWriter<W> {
        VcdWriter::new(writer, self)
    }

    fn next_wire_id(&self) -> IdCode {
        self.wires.first()
            .map(|wire| wire.id.next())
            .unwrap_or(IdCode::FIRST)
    }
}

pub struct VcdWriter<W: IoWrite> {
    inner: vcd::Writer<W>,
    config: VcdWriterConfig,
    time: usize,
}

impl<W: IoWrite> VcdWriter<W> {
    fn new(writer: W, options: VcdWriterConfig) -> Self {
        Self {
            inner: vcd::Writer::new(writer),
            config: options,
            time: 0,
        }
    }

    /// Writes everything that should precede the data dump section of a VCD
    fn write_preamble(&mut self) -> IoResult<()> {
        for wire in &self.config.wires {
            self.inner.var_def(vcd::VarType::Wire, wire.width as u32, wire.id, &wire.name, None)?;
        }

        // Header
        self.inner.timescale(1, vcd::TimescaleUnit::US)?;
        self.inner.add_module(&self.config.module)?;
        for wire in &self.config.wires {
            self.inner.add_wire(wire.width as u32, &wire.name)?;
        }
        self.inner.upscope()?;
        self.inner.enddefinitions()?;

        // Initialize all variables to unknown
        self.inner.begin(SimulationCommand::Dumpvars)?;
        for wire in &self.config.wires {
            self.inner.change_vector(wire.id, wire.unknown())?;
        }
        self.inner.end()?;

        Ok(())
    }

    pub fn try_write_cluster(&mut self, signals: &SignalCluster) -> IoResult<()> {
        if signals.cluster.len() != self.config.wires.len() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("Expected {} signals, received {}", self.config.wires.len(), signals.cluster.len()),
            ));
        }

        if self.time == 0 {
            self.write_preamble()?;
        }

        let max_sample_count = signals
            .cluster
            .iter()
            .map(|b| b.samples.len())
            .max()
            .ok_or(std::io::ErrorKind::InvalidData)?;

        for t in (self.time..).take(max_sample_count) {
            self.inner.timestamp(t as u64)?;

            for (index, wire) in self.config.wires.iter().enumerate() {
                if let Some(signal) = signals.cluster.get(index) {
                    let current_vector = signal
                        .samples
                        .get(t)
                        .ok_or(std::io::ErrorKind::InvalidData)?
                        .iter()
                        .map(|b| b.then_some(VcdValue::V1).unwrap_or(VcdValue::V0));

                    self.inner.change_vector(wire.id, current_vector)?;
                }
            }
        }
        self.time += max_sample_count;

        Ok(())
    }
}
