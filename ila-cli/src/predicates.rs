use std::io::{ErrorKind, Read as IoRead, Result as IoResult, Write as IoWrite};

use crate::{
    cli::CommandOutput,
    cli_registers::IlaRegisters,
    communication::{perform_register_operation, ReadWrite, RegisterOutput, SignalCluster},
    config::IlaConfig,
};

/// Different operations for how multiple predicates are combined
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum PredicateOperation {
    And = 0,
    Or = 1,
}

impl CommandOutput for PredicateOperation {
    fn command_output(&self) -> String {
        match self {
            PredicateOperation::And => "and",
            PredicateOperation::Or => "or",
        }
        .into()
    }
}

impl TryFrom<u32> for PredicateOperation {
    type Error = ();

    fn try_from(value: u32) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(PredicateOperation::And),
            1 => Ok(PredicateOperation::Or),
            _ => Err(()),
        }
    }
}

/// The places within the ILA where the predicate logic can be applied too
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PredicateTarget {
    Trigger,
    Capture,
}

/// The configuration for predicates
#[derive(Debug, Clone)]
pub struct IlaPredicate {
    /// Where in the ILA these predicates will take effect
    pub target: PredicateTarget,
    /// How this configuration should combine its predicates
    pub operation: PredicateOperation,
    /// Which predicates are active
    pub predicate_select: u32,
    /// Filter out bits before comparison
    pub mask: SignalCluster,
    /// The comparison value
    pub compare: SignalCluster,
}

impl IlaPredicate {
    /// Creates an up-to-date `IlaPredicate` by grabbing the necessary information from the ILA
    pub fn from_ila<T>(
        medium: &mut T,
        ila: &IlaConfig,
        target: PredicateTarget,
    ) -> IoResult<IlaPredicate>
    where
        T: IoWrite + IoRead,
    {
        let sample_word_width = ila.signals.transaction_bit_count().div_ceil(32);

        match target {
            PredicateTarget::Trigger => {
                let RegisterOutput::TriggerOp(operation) = perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::TriggerOp(ReadWrite::Read(())),
                )?
                else {
                    return Err(ErrorKind::InvalidData.into());
                };
                let RegisterOutput::TriggerSelect(selected) = perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::TriggerSelect(ReadWrite::Read(())),
                )?
                else {
                    return Err(ErrorKind::InvalidData.into());
                };
                let RegisterOutput::TriggerMask(mask) = perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::TriggerMask(ReadWrite::Read(sample_word_width as u32)),
                )?
                else {
                    return Err(ErrorKind::InvalidData.into());
                };
                let RegisterOutput::TriggerCompare(compare) = perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::TriggerCompare(ReadWrite::Read(sample_word_width as u32)),
                )?
                else {
                    return Err(ErrorKind::InvalidData.into());
                };

                Ok(IlaPredicate {
                    target: PredicateTarget::Trigger,
                    operation,
                    predicate_select: selected,
                    mask,
                    compare,
                })
            }
            PredicateTarget::Capture => {
                let RegisterOutput::CaptureOp(operation) = perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::CaptureOp(ReadWrite::Read(())),
                )?
                else {
                    return Err(ErrorKind::InvalidData.into());
                };
                let RegisterOutput::CaptureSelect(selected) = perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::CaptureSelect(ReadWrite::Read(())),
                )?
                else {
                    return Err(ErrorKind::InvalidData.into());
                };
                let RegisterOutput::CaptureMask(mask) = perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::CaptureMask(ReadWrite::Read(sample_word_width as u32)),
                )?
                else {
                    return Err(ErrorKind::InvalidData.into());
                };
                let RegisterOutput::CaptureCompare(compare) = perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::CaptureCompare(ReadWrite::Read(sample_word_width as u32)),
                )?
                else {
                    return Err(ErrorKind::InvalidData.into());
                };

                Ok(IlaPredicate {
                    target: PredicateTarget::Capture,
                    operation,
                    predicate_select: selected,
                    mask,
                    compare,
                })
            }
        }
    }

    /// Attempts to bring the ILA up to date with the current configuration for the ILA predicate
    pub fn update_ila<T>(&self, medium: &mut T, ila: &IlaConfig) -> IoResult<()>
    where
        T: IoWrite + IoRead,
    {
        match self.target {
            PredicateTarget::Trigger => {
                perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::TriggerSelect(ReadWrite::Write(self.predicate_select)),
                )
                .and(perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::TriggerOp(ReadWrite::Write(self.operation)),
                ))
                .and(perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::TriggerMask(ReadWrite::Write(self.mask.to_data())),
                ))
                .and(perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::TriggerCompare(ReadWrite::Write(self.compare.to_data())),
                ))
                // We don't care for the output, just about if it errors or not, so we map it to ()
                .map(|_| ())
            }
            PredicateTarget::Capture => {
                perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::CaptureSelect(ReadWrite::Write(self.predicate_select)),
                )
                .and(perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::CaptureOp(ReadWrite::Write(self.operation)),
                ))
                .and(perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::CaptureMask(ReadWrite::Write(self.mask.to_data())),
                ))
                .and(perform_register_operation(
                    medium,
                    ila,
                    &IlaRegisters::CaptureCompare(ReadWrite::Write(self.compare.to_data())),
                ))
                // We don't care for the output, just about if it errors or not, so we map it to ()
                .map(|_| ())
            }
        }
    }
}
