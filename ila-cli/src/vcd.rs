use std::{io::Result as IoResult, path::Path};
use vcd::{IdCode, SimulationCommand, Value as VcdValue};

use crate::{communication::{Signal, SignalCluster}, config::IlaConfig};

type VcdWriter = vcd::Writer<std::io::BufWriter<std::fs::File>>;
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
    fn from_vcd(vcd_writer: &mut VcdWriter, signal: &Signal) -> IoResult<VcdSignal> {
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
/// * `path` - Path to the write to, will overwrite any file already in place
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
