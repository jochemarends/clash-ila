use std::fmt::Display;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::communication::{RegisterOutput, perform_buffer_reads, perform_register_operation};
use crate::cli_registers::IlaRegisters;
use crate::config::ConfigMethod;
use crate::export::ExportCluster;
use crate::tui::TuiSession;
use crate::vcd::write_to_vcd;
use clap::{arg, Args, ValueEnum};
use serialport::SerialPort;

use crate::cli_registers::RegisterSubcommand;

/// Implementing this trait implies the data type can be converted to a command line output
pub trait CommandOutput {
    fn command_output(&self) -> String;
}

/// Simple trait to indicate this is a valid subcommand with arguments
pub trait ParseSubcommand {
    fn parse(self);
}

/// Check if an user given port path is valid and readable, intended to be used within a CLI-like
/// context
///
/// Returns a readable serial port on success
///
/// # Panics
///
/// Panics if the user provided an incorrect path or other IO errors accure
pub fn find_specified_port(check: &Path, baud: u32) -> Box<dyn SerialPort> {
    let ports = match serialport::available_ports() {
        Ok(ports) => ports,
        Err(err) => {
            println!("Unable to qeury serial port information;");
            println!("Kind: {:?}", err.kind);
            println!("Reason: {}", err.description);
            panic!("Unable to query serial port information.")
        }
    };

    let check_path = check.display().to_string();

    let valid_port = ports
        .into_iter()
        .find(|port| port.port_name == check_path)
        .expect("Provided path is not a valid serial port.");

    serialport::new(valid_port.port_name, baud)
        .timeout(Duration::from_secs(1))
        .open()
        .expect("Unable to open serial port (maybe busy?)")
}

#[derive(Args, Debug)]
pub struct RegisterArgs {
    #[arg(short, long, help = "Path to serial port to use")]
    port: PathBuf,

    #[arg(short, long, default_value_t = 115200, help = "Sets baud rate")]
    baud: u32,

    #[command(flatten)]
    config: ConfigMethod,

    #[command(subcommand)]
    register: RegisterSubcommand,
}

impl ParseSubcommand for RegisterArgs {
    fn parse(self) {
        let mut tx_port = find_specified_port(&self.port, self.baud);
        let config = self
            .config
            .get_config()
            .unwrap_or_else(|_| panic!("File at {:?} contained errors", &self.config));

        let output = perform_register_operation(&mut tx_port, &config, &self.register.into());
        match output {
            Ok(output) => println!("{}", output.command_output()),
            Err(err) => panic!("{err}"),
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ExportType {
    Vcd,
    Csv,
    Json,
}

impl Display for ExportType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            ExportType::Vcd => "vcd",
            ExportType::Csv => "csv",
            ExportType::Json => "json",
        })
    }
}

#[derive(Args, Debug)]
pub struct ExportArgs {
    #[arg(short, long, help = "Path to serial port to use")]
    port: PathBuf,

    #[arg(short, long, default_value_t = 115200, help = "Sets baud rate")]
    baud: u32,

    #[arg(short = 's', long, default_value_t = 0, help = "Start index to grab data from (inclusive)")]
    start_index: u32,

    #[arg(short = 'x', long, help = "End index to grab data from (exclusive)")]
    end_index: u32,

    #[command(flatten)]
    config: ConfigMethod,

    #[arg(short, long, help = "Name of the exported file")]
    name: PathBuf,

    #[arg(short, long, default_value_t = ExportType::Vcd, help = "What file format to export to")]
    export: ExportType,
}

impl ParseSubcommand for ExportArgs {
    fn parse(self) {
        let mut tx_port = find_specified_port(&self.port, self.baud);
        let config = self
            .config
            .get_config()
            .unwrap_or_else(|_| panic!("File at {:?} contained errors", &self.config));

        let cluster = match perform_buffer_reads(
            &mut tx_port,
            &config,
            self.start_index..self.end_index
        ) {
            Ok(RegisterOutput::BufferContent(cluster)) => cluster,
            Ok(_) => panic!("Unexpected output when reading buffer"),
            Err(err) => panic!("Error: {err}"),
        };

        match self.export {
            ExportType::Vcd => write_to_vcd(&cluster, &config, self.name)
                .expect("Unable to write to VCD file"),
            ExportType::Csv => {
                let export: ExportCluster = cluster.into();
                let mut writer = csv::WriterBuilder::new()
                    .from_path(self.name)
                    .expect("Unable to open path to file");

                for signal in export.signals {
                    writer.serialize(signal)
                        .expect("Unable to write signal to CSV file");
                }

                writer.flush()
                    .expect("Unable to flush written data to CSV file");
            },
            ExportType::Json => {
                let export: ExportCluster = cluster.into();
                let serialized = serde_json::to_string(&export)
                    .expect("Unable to serialize signals to JSON string");
                std::fs::write(self.name, serialized)
                    .expect("Unable to write serialized data to file");
            },
        };
    }
}

#[derive(Args, Debug)]
pub struct MonitorArgs {
    #[arg(short, long, help = "Path to serial port to use")]
    port: PathBuf,

    #[arg(short, long, default_value_t = 115200, help = "Sets baud rate")]
    baud: u32,

    #[arg(
        short = 'l',
        long = "line",
        default_value_t = 16,
        help = "Amount of bytes displayed per line"
    )]
    max_per_line: u32,

    #[arg(
        short = 's',
        long = "space",
        default_value_t = 2,
        help = "Amount of bytes displayed per space"
    )]
    max_per_space: u32,
}

impl ParseSubcommand for MonitorArgs {
    fn parse(self) {
        let port = find_specified_port(&self.port, self.baud);
        monitor_port(port, self)
    }
}

#[derive(Args, Debug)]
pub struct TuiArgs {
    #[arg(short, long, help = "Path to serial port to use")]
    port: PathBuf,

    #[command(flatten)]
    config: ConfigMethod,

    #[arg(short, long, default_value_t = 115200, help = "Sets baud rate")]
    baud: u32,
}

impl ParseSubcommand for TuiArgs {
    fn parse(self) {
        let mut tx_port = find_specified_port(&self.port, self.baud);
        let config = self
            .config
            .get_config()
            .unwrap_or_else(|_| panic!("File at {:?} contained errors", &self.config));

        match perform_register_operation(&mut tx_port, &config, &IlaRegisters::Hash(config.hash)) {
            Ok(RegisterOutput::Hash(true)) => {}
            Ok(_) => {
                println!("Provided config hash and ILA hash do not match!");
                return;
            }
            Err(err) => {
                println!("Failed to send ILA: {err}");
                panic!("Failed to check for the ILA hash");
            }
        }

        let Ok(mut session) = TuiSession::new(&config, &self.port) else {
            return;
        };
        session.main_loop(tx_port);
    }
}

/// `monitor` CLI handler
pub fn monitor_port(port: Box<dyn SerialPort>, args: MonitorArgs) {
    let mut addr = 0;
    let mut wrote = 0;

    println!(
        "Monitoring on {}",
        port.name().unwrap_or(String::from("<unknown>"))
    );

    for byte in port.bytes() {
        let Ok(byte) = byte else { continue };

        if wrote == 0 {
            print!("{addr:#08x}: ");
            addr += args.max_per_line;
        }

        print!("{byte:02x}");

        wrote += 1;
        if wrote % args.max_per_space == 0 {
            print!(" ");
        }
        if wrote == args.max_per_line {
            wrote = 0;
            println!(); // Using ln rather than \n to keep cross-compatibility
        }

        // If we can't flush, we can try again next byte
        // I want SOME sort of indicator for this though, but that's difficult
        let _ = std::io::stdout().flush();
    }
}
