//! Module for integrating the Lua programming language with the ILA-CLI.

use std::time::Duration;

use mlua::prelude::*;
use num::BigUint;

use crate::communication::{biguint_to_sample, Signal, SignalCluster};
use crate::config::{IlaConfig, IlaSignal};
use crate::predicates::{IlaPredicate, PredicateOperation, PredicateTarget};
use crate::predicates_tui::NumericState;
use crate::tui::TuiAction;

#[derive(Debug, Clone)]
pub struct LuaVm {
    vm: Lua,
}

impl FromLua for PredicateOperation {
    fn from_lua(value: LuaValue, lua: &Lua) -> LuaResult<Self> {
        let s = String::from_lua(value, lua)?;
        match s.as_str() {
            "or" => Ok(Self::Or),
            "and" => Ok(Self::And),
            _ => Err(LuaError::runtime(format!(
                "invalid predicate operation: `{s}`, expected `and` or `or`"
            ))),
        }
    }
}

fn cluster_from_lua(value: LuaValue, signals: &Vec<IlaSignal>) -> LuaResult<SignalCluster> {
    let LuaValue::Table(sample) = value else {
        return Err(LuaError::RuntimeError(
            format!("{:?} {}", value, "expected a table".to_string()),
        ));
    };

    let cluster: Vec<_> = (1..=sample.len().unwrap())
        .map(|index| sample.get::<String>(index).unwrap())
        .zip(signals)
        .filter_map(|(input, signal)| {
            NumericState::immediate_parse(&input)
                .ok()
                .map(|n| (n, signal))
        })
        .filter(|(n, signal)| {
            *n < BigUint::new(vec![2]).pow(signal.width as u32)
        })
        .map(|(n, signal)| Signal {
            name: signal.name.clone(),
            width: signal.width,
            samples: vec![biguint_to_sample(&n, signal.width)],
        })
        .collect();

    Ok(SignalCluster {
        cluster,
        timestamp: Duration::ZERO,
    })
}

fn predicate_from_lua(value: LuaValue, target: PredicateTarget, inputs: &Vec<IlaSignal>) -> LuaResult<IlaPredicate> {
    let LuaValue::Table(value) = value else {
        return Err(LuaError::RuntimeError(
            "expected a table".to_string(),
        ));
    };

    Ok(IlaPredicate {
        target,
        operation: value.get::<PredicateOperation>("combine")?,
        mask: cluster_from_lua(value.get("masks")?, inputs)?,
        compare: cluster_from_lua(value.get("compares")?, inputs)?,
        predicate_select: 0,
    })
}

impl IntoLua for IlaConfig {
    fn into_lua(self, lua: &Lua) -> LuaResult<LuaValue> {
        let config = lua.create_table()?;
        config.set("toplevel", self.toplevel.clone())?;
        config.set("hash", self.hash.clone())?;
        config.set("buffer_size", self.buffer_size)?;

        let inputs: Vec<LuaTable> = self
            .signals
            .iter()
            .map(|signal| {
                let input = lua.create_table()?;
                input.set("name", signal.name.clone())?;
                input.set("width", signal.width)?;
                Ok(input)
            })
            .collect::<LuaResult<_>>()?;

        config.set("inputs", lua.create_sequence_from(inputs)?)?;

        Ok(LuaValue::Table(config))
    }
}

impl LuaVm {
    /// Constructs a new Lua VM with functions and variables for the ILA.
    pub fn new(config: &IlaConfig) -> LuaResult<Self> {
        let vm = Lua::new();

        let LuaValue::Table(ila) = config.clone().into_lua(&vm)? else {
            return Err(LuaError::RuntimeError(
                "expected `ila` to be a table".to_string(),
            ));
        };

        vm.set_app_data::<Vec<TuiAction>>(Vec::new());

        vm.globals().set(
            "print",
            vm.create_function(|lua, s: String| {
                if let Some(mut actions) = lua.app_data_mut::<Vec<TuiAction>>() {
                    actions.push(TuiAction::Print(s));
                }
                Ok(())
            })?,
        )?;

        let config = config.clone();

        ila.set(
            "config",
            vm.create_function(move |lua, table: mlua::Table| {
                let mut actions = lua.app_data_mut::<Vec<TuiAction>>().unwrap();

                if let Ok(true) = table.contains_key("trigger_point") {
                    actions.push(TuiAction::SetTriggerPoint(table.get::<u32>("trigger_point")?));
                }

                if let Ok(true) = table.contains_key("auto_rearm") {
                    actions.push(TuiAction::SetAutoRearm(table.get::<bool>("auto_rearm")?));
                }

                if let Ok(true) = table.contains_key("trigger") {
                    let predicate = predicate_from_lua(table.get("trigger")?, PredicateTarget::Trigger, &config.signals)?;
                    actions.push(TuiAction::SetPredicate(predicate));
                }

                if let Ok(true) = table.contains_key("capture") {
                    let predicate = predicate_from_lua(table.get("capture")?, PredicateTarget::Capture, &config.signals)?;
                    actions.push(TuiAction::SetPredicate(predicate));
                }

                Ok(())
            })?,
        )?;

        vm.globals().set("ila", LuaValue::Table(ila))?;

        Ok(LuaVm { vm })
    }

    /// Executes Lua code.
    pub fn exec(&mut self, code: &str) -> LuaResult<Vec<TuiAction>> {
        self.vm.load(code).exec()?;

        let actions = self
            .vm
            .app_data_mut::<Vec<TuiAction>>()
            .map(|mut data| std::mem::take(&mut *data))
            .unwrap_or_default();

        Ok(actions)
    }
}
