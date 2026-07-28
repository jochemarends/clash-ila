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

    let mut root = VcdModule::new(config.toplevel.clone());

    for signal in &signals.cluster {
        root = root.add_wire(signal.name.to_string(), signal.width);
    }

    let mut vcd_writer = root.writer(writer);
    vcd_writer.write_preamble()?;
    vcd_writer.write_cluster(&signals)?;
    vcd_writer.flush()
}

fn unknown_bits(width: usize) -> impl Iterator<Item = vcd::Value> {
    std::iter::repeat(vcd::Value::X).take(width)
}

#[derive(Debug, Clone)]
enum VcdItem {
    Wire { name: String, width: usize },
    Module(VcdModule),
}

impl VcdItem {
    /// Performs a pre-order depth-first fold over this item and its descendants.
    fn fold<'a, B, F>(&'a self, init: B, f: &mut F) -> B
    where
        F: FnMut(B, &'a VcdItem) -> B,
    {
        let acc = f(init, &self);
        if let VcdItem::Module(module) = self {
            module.items.iter().fold(acc, |acc, item| item.fold(acc, f))
        } else {
            acc
        }
    }

    /// Performs a pre-order depth-first fold over this item and its descendants as long as the fold
    /// function does not return an error.
    fn try_fold<'a, B, F, Error>(&'a self, init: B, f: &mut F) -> Result<B, Error>
    where
        F: FnMut(B, &'a VcdItem) -> Result<B, Error>,
    {
        f(init, &self).and_then(|acc| {
            if let VcdItem::Module(module) = self {
                module
                    .items
                    .iter()
                    .try_fold(acc, |acc, item| item.try_fold(acc, f))
            } else {
                Ok(acc)
            }
        })
    }
}

/// Configuration builder for [`VcdWriter`].
///
/// Specifies the module and variables that a [`VcdWriter`] will emit.
///
/// # Examples
///
/// ```
/// use std::fs::File;
/// use crate::vcd::VcdModule;
///
/// let file = File::create("dump.vcd").unwrap();
///
/// let mut vcd_writer = VcdModule::new("mux".to_string())
///     .add_wire("a".to_string(), 8)
///     .add_wire("b".to_string(), 8)
///     .add_wire("select".to_string(), 1)
///     .add_wire("output".to_string(), 8)
///     .writer(file);
/// ```
#[derive(Debug, Clone)]
pub struct VcdModule {
    name: String,
    items: Vec<VcdItem>,
}

impl VcdModule {
    /// Constructs a new [`VcdModule`] with the specified name.
    pub fn new(name: String) -> Self {
        Self {
            name: name,
            items: Vec::new(),
        }
    }

    /// Adds a wire with the given name and width.
    ///
    /// The wire will be added to the module associated with this configuration.
    pub fn add_wire(mut self, name: String, width: usize) -> Self {
        self.items.push(VcdItem::Wire { name, width });
        self
    }

    /// Adds a submodule.
    #[allow(unused)]
    pub fn add_module(mut self, module: VcdModule) -> Self {
        self.items.push(VcdItem::Module(module));
        self
    }

    /// Constructs a new [`VcdWriter`] for this module.
    pub fn writer<W: IoWrite>(self, writer: W) -> VcdWriter<W> {
        VcdWriter::new(writer, self)
    }

    /// Performs a pre-order depth-first fold over the items in this VCD module.
    fn fold<'a, B, F>(&'a self, init: B, f: &mut F) -> B
    where
        F: FnMut(B, &'a VcdItem) -> B,
    {
        self.items.iter().fold(init, |acc, item| item.fold(acc, f))
    }

    /// Performs a pre-order depth-first fold over the items in this VCD module as long as the fold
    /// function does not return an error.
    fn try_fold<'a, B, F, Error>(&'a self, init: B, f: &mut F) -> Result<B, Error>
    where
        F: FnMut(B, &'a VcdItem) -> Result<B, Error>,
    {
        self.items
            .iter()
            .try_fold(init, |acc, item| item.try_fold(acc, f))
    }
}

/// Wraps a [`std::io::Write`] for incrementally writing [`SignalCluster`]s as VCD.
pub struct VcdWriter<W: IoWrite> {
    inner: vcd::Writer<W>,
    root: VcdModule,
    time: u64,
}

impl<W: IoWrite> VcdWriter<W> {
    /// Construct a new VCD writer with the given writer and configuration.
    fn new(writer: W, config: VcdModule) -> Self {
        Self {
            inner: vcd::Writer::new(writer),
            root: config,
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

        Self::add_module(&mut self.inner, &self.root, IdCode::FIRST)?;

        self.inner.enddefinitions()?;

        let _ = self.root.fold(0, &mut |acc, item| {
            acc + matches!(item, VcdItem::Wire { .. }) as usize
        });

        // Initialize all variables to unknown
        self.inner.begin(SimulationCommand::Dumpvars)?;

        self.root
            .try_fold(IdCode::FIRST, &mut |code, item| -> IoResult<IdCode> {
                match item {
                    VcdItem::Wire { width, .. } => {
                        self.inner.change_vector(code, unknown_bits(*width))?;
                        Ok(code.next())
                    }
                    _ => Ok(code),
                }
            })?;

        self.inner.end()
    }

    fn add_module(
        inner: &mut vcd::Writer<W>,
        module: &VcdModule,
        mut id: IdCode,
    ) -> IoResult<IdCode> {
        inner.add_module(&module.name)?;

        for item in &module.items {
            id = match item {
                VcdItem::Wire { name, width } => inner
                    .var_def(vcd::VarType::Wire, *width as u32, id, name, None)
                    .map(|_| id.next()),
                VcdItem::Module(module) => Self::add_module(inner, module, id),
            }?;
        }

        inner.upscope()?;
        Ok(id)
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
    /// specified in this writer's module (including submodules).
    pub fn write_cluster(&mut self, signals: &SignalCluster) -> IoResult<()> {
        let wire_count = self.root.fold(0, &mut |acc, item| {
            acc + matches!(item, VcdItem::Wire { .. }) as usize
        });

        if signals.cluster.len() != wire_count {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!(
                    "VCD write error in module '{}': expected {} signals, got {}",
                    self.root.name,
                    wire_count,
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

            let _ = self.root.try_fold(
                (IdCode::FIRST, signals.cluster.iter()),
                &mut |(id, mut signals), item| -> IoResult<_> {
                    if let VcdItem::Wire { width, .. } = item {
                        let signal = signals.next().unwrap();

                        if let Some(sample) = signal.samples.get(index) {
                            let current_vector = sample
                                .iter()
                                .map(|b| b.then_some(vcd::Value::V1).unwrap_or(vcd::Value::V0));

                            self.inner.change_vector(id, current_vector)?;
                        } else {
                            self.inner.change_vector(id, unknown_bits(*width))?;
                        }

                        Ok((id.next(), signals))
                    } else {
                        Ok((id, signals))
                    }
                },
            )?;
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

    /// Reset the timestamp.
    pub fn reset_timestamp(&mut self) {
        self.time = 0;
    }
}

#[cfg(test)]
mod tests {
    use bitvec::prelude::*;

    use crate::{communication::Signal};

    use super::*;

    #[test]
    #[should_panic]
    fn write_empty_cluster() {
        let mut writer = VcdModule::new("mux".to_string())
            .add_wire("a".to_string(), 8)
            .add_wire("b".to_string(), 8)
            .add_wire("select".to_string(), 1)
            .add_wire("output".to_string(), 8)
            .writer(std::io::sink());

        let cluster = SignalCluster {
            cluster: vec![],
            timestamp: std::time::Duration::ZERO,
        };

        writer.write_cluster(&cluster).unwrap();
    }

    #[test]
    // After writing a preamble, the VCD should contain a module with the correct name and
    // variables.
    fn write_preamble() {
        let mut writer = VcdModule::new("toplevel".to_string())
            .add_wire("a".to_string(), 1)
            .add_wire("b".to_string(), 2)
            .add_wire("c".to_string(), 0)
            .writer(Vec::<u8>::new());

        writer.write_preamble().unwrap();

        let cursor = std::io::Cursor::new(writer.writer_mut());

        let mut parser = vcd::Parser::new(cursor);

        let header = parser.parse_header().unwrap();

        let scope = match header.items.as_slice() {
            [vcd::ScopeItem::Scope(scope)] => scope,
            _ => panic!("expected a single scope"),
        };

        let mut vars: Vec<_> = scope.items.iter().filter_map(|item| match item {
            vcd::ScopeItem::Var(var) => Some((&var.reference, var.size)),
            _ => None,
        }).collect();

        assert_eq!(scope.identifier, "toplevel");

        assert_eq!(
            vars.sort(),
            vec![
                ("a", 1),
                ("b", 2),
                ("c", 0),
            ].sort(),
        );
    }

    // When writing multiple signals and not all have an equal number of samples, the ones with
    // fewer samples should get with undefined.
    #[test]
    fn pad_shorter_samples() {
        let mut writer = VcdModule::new("toplevel".to_string())
            .add_wire("a".to_string(), 1)
            .add_wire("b".to_string(), 1)
            .writer(Vec::<u8>::new());

        // Make a cluster where one signal has more samples.
        let cluster = SignalCluster {
            cluster: vec![
                Signal {
                    name: String::new(),
                    width: 1,
                    samples: vec![
                        bitvec![u8, Msb0; 1],
                        bitvec![u8, Msb0; 1],
                        bitvec![u8, Msb0; 0],
                    ],
                },
                Signal {
                    name: String::new(),
                    width: 1,
                    samples: vec![
                        bitvec![u8, Msb0; 1],
                    ],
                },
            ],
            timestamp: std::time::Duration::ZERO,
        };

        writer.write_cluster(&cluster).unwrap();

        let cursor = std::io::Cursor::new(writer.writer_mut());

        // Parse the generated VCD.
        let parser = vcd::Parser::new(cursor);

        type Samples = Vec<(vcd::IdCode, vcd::Vector)>;

        let (samples_a, samples_b): (Samples, Samples) = parser.into_iter()
            .filter_map(|cmd| match cmd {
                Ok(vcd::Command::ChangeVector(id, vec)) => Some((id, vec)),
                _ => None,
            })
            .partition(|(id, _)| *id == IdCode::FIRST);

        assert_eq!(
            samples_a.iter().map(|(_, sample)| sample.iter().collect::<Vec<_>>()).collect::<Vec<_>>(),
            vec![
                vec![vcd::Value::V1],
                vec![vcd::Value::V1],
                vec![vcd::Value::V0],
            ],
        );

        assert_eq!(
            samples_b.iter().map(|(_, sample)| sample.iter().collect::<Vec<_>>()).collect::<Vec<_>>(),
            vec![
                vec![vcd::Value::V1],
                vec![vcd::Value::X],
                vec![vcd::Value::X],
            ],
        );
    }
}
