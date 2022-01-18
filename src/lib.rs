mod timer;

#[macro_use]
extern crate lazy_static;

use crate::timer::Timer;
use ckb_vm::{machine::asm::AsmMachine, CoreMachine};
use nix::sys::signal;
use std::ops::Deref;
use std::os::raw::c_int;
use std::pin::Pin;
use std::sync::Mutex;

lazy_static! {
    pub static ref PROFILER: Mutex<Profiler> = Mutex::new(Profiler {
        fname: "".to_string(),
        machine: 0,
        timer: None,
    });
}

pub struct Profiler {
    fname: String,
    machine: usize,
    timer: Option<Timer>,
}

extern "C" fn perf_signal_handler(_signal: c_int) {
    let profiler = PROFILER.lock().expect("Mutex lock failure");
    if !profiler.is_start() {
        return;
    }

    let machine = unsafe { &*(profiler.machine as *const AsmMachine) as &AsmMachine };

    // TODO: inspect and record frames. For now, we can assume that frame pointer is present
    println!("PC: {:x}", machine.machine.pc());
}

impl Profiler {
    pub fn is_start(&self) -> bool {
        self.timer.is_some()
    }

    pub fn start(&mut self, fname: &str, machine: &Pin<Box<AsmMachine>>) -> Result<(), String> {
        if self.is_start() {
            return Err("Profiler already started!".to_string());
        }
        self.fname = fname.to_string();
        self.machine = machine.deref() as *const AsmMachine as usize;

        // install signal handler
        let handler = signal::SigHandler::Handler(perf_signal_handler);
        let sigaction = signal::SigAction::new(
            handler,
            signal::SaFlags::SA_RESTART,
            signal::SigSet::empty(),
        );
        unsafe { signal::sigaction(signal::SIGPROF, &sigaction) }
            .map_err(|e| format!("sigaction install error: {}", e))?;

        self.timer = Some(Timer::new(99));

        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), String> {
        if !self.is_start() {
            return Err("Profiler not started!".to_string());
        }

        // uninstall signal handler
        let handler = signal::SigHandler::SigIgn;
        unsafe { signal::signal(signal::SIGPROF, handler) }
            .map_err(|e| format!("sigaction uninstall error: {}", e))?;

        // TODO: write profiling data to file fname

        self.fname = "".to_string();
        self.machine = 0;
        self.timer = None;

        Ok(())
    }
}
