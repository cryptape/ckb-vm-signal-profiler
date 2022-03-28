mod frames;
mod protos;
mod timer;

#[macro_use]
extern crate lazy_static;

use crate::{
    frames::{Frame, Report, Symbol},
    timer::Timer,
};
use ckb_vm::{machine::asm::AsmMachine, Bytes, CoreMachine};
use nix::sys::signal;
use protobuf::Message;
use std::borrow::Cow;
use std::fs;
use std::ops::{Deref, DerefMut};
use std::os::raw::c_int;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

lazy_static! {
    static ref PROFILER: Mutex<Option<Profiler>> = Mutex::new(None);
}

type Addr2LineEndianReader =
    addr2line::gimli::EndianReader<addr2line::gimli::RunTimeEndian, Arc<[u8]>>;
type Addr2LineContext = addr2line::Context<Addr2LineEndianReader>;
type Addr2LineFrameIter<'a> = addr2line::FrameIter<'a, Addr2LineEndianReader>;

struct DebugContext {
    addr_context: Addr2LineContext,
    debug_frame: addr2line::gimli::DebugFrame<Addr2LineEndianReader>,
}

struct Profiler {
    fname: String,
    machine: usize,
    context: DebugContext,
    // Drop behavior is enough for timer
    #[allow(dead_code)]
    timer: Timer,
    report: Report,
}

// A temporary work till frame is properly implemented
fn extract_frame(pc: u64, context: &DebugContext) -> Frame {
    let addr_context = &context.addr_context;
    let mut file = None;
    let mut line = None;

    // TODO: trace frame to reveal the whole stack
    let loc = addr_context.find_location(pc).unwrap();
    if let Some(loc) = loc {
        file = Some(loc.file.as_ref().unwrap().to_string());
        if let Some(loc_line) = loc.line {
            line = Some(loc_line);
        }
    }
    let mut frame_iter = addr_context.find_frames(pc).unwrap();
    let sprint_fun = |frame_iter: &mut Addr2LineFrameIter| {
        let mut s = String::from("??");
        loop {
            if let Some(data) = frame_iter.next().unwrap() {
                if let Some(function) = data.function {
                    s = String::from(addr2line::demangle_auto(
                        Cow::from(function.raw_name().unwrap()),
                        function.language,
                    ));
                    continue;
                }
            }
            break;
        }
        s
    };
    let func = sprint_fun(&mut frame_iter);

    let symbol = Symbol {
        name: Some(func),
        line,
        file,
    };
    let mut frame = Frame::default();
    frame.stacks.push(symbol);
    frame
}

extern "C" fn perf_signal_handler(_signal: c_int) {
    let mut profiler = PROFILER.lock().expect("Mutex lock failure");
    if let Some(profiler) = profiler.deref_mut() {
        let machine = unsafe { &*(profiler.machine as *const AsmMachine) as &AsmMachine };

        let pc = *machine.machine.pc();
        let frame = extract_frame(pc, &profiler.context);

        profiler.report.record(&frame);
    }
}

pub fn is_profiler_started() -> bool {
    PROFILER.lock().expect("Mutex lock failure").is_some()
}

fn build_context(program: &Bytes) -> Result<DebugContext, String> {
    use addr2line::object::{Object, ObjectSection};

    // Adapted from https://github.com/gimli-rs/addr2line/blob/fc2de9f47ae513f5a54448167b476ff50f07dca6/src/lib.rs#L87-L148
    // for working with gimli::EndianArcSlice type
    let file = addr2line::object::File::parse(program.as_ref())
        .map_err(|e| format!("object parsing error: {}", e))?;

    let dwarf = addr2line::gimli::Dwarf::load(|id| {
        let data = file
            .section_by_name(id.name())
            .and_then(|section| section.uncompressed_data().ok())
            .unwrap_or(Cow::Borrowed(&[]));
        Ok(addr2line::gimli::EndianArcSlice::new(
            Arc::from(&*data),
            addr2line::gimli::RunTimeEndian::Little,
        ))
    })
    .map_err(|e: addr2line::gimli::Error| format!("dwarf load error: {}", e))?;

    let addr_context = Addr2LineContext::from_dwarf(dwarf)
        .map_err(|e| format!("context creation error: {}", e))?;

    let debug_frame_section = file
        .section_by_name(addr2line::gimli::SectionId::DebugFrame.name())
        .and_then(|s| s.uncompressed_data().ok())
        .ok_or_else(|| "Provided binary is missing .debug_frame section!".to_string())?;
    let debug_frame_reader = Addr2LineEndianReader::new(
        Arc::from(&*debug_frame_section),
        addr2line::gimli::RunTimeEndian::Little,
    );

    Ok(DebugContext {
        addr_context,
        debug_frame: debug_frame_reader.into(),
    })
}

pub fn start_profiler(
    fname: &str,
    machine: &Pin<Box<AsmMachine>>,
    program: &Bytes,
    frequency_per_sec: i32,
) -> Result<(), String> {
    if is_profiler_started() {
        return Err("Profiler already started!".to_string());
    }

    let context = build_context(program)?;

    // install signal handler
    let handler = signal::SigHandler::Handler(perf_signal_handler);
    let sigaction = signal::SigAction::new(
        handler,
        signal::SaFlags::SA_RESTART,
        signal::SigSet::empty(),
    );
    unsafe { signal::sigaction(signal::SIGPROF, &sigaction) }
        .map_err(|e| format!("sigaction install error: {}", e))?;

    let profiler = Profiler {
        fname: fname.to_string(),
        machine: machine.deref() as *const AsmMachine as usize,
        context,
        timer: Timer::new(frequency_per_sec),
        report: Report::default(),
    };

    *(PROFILER.lock().expect("Mutex lock failure")) = Some(profiler);

    Ok(())
}

pub fn stop_profiler() -> Result<(), String> {
    let mut profiler = PROFILER.lock().expect("Mutex lock failure");
    if profiler.is_none() {
        return Err("Profiler not started!".to_string());
    }
    // save profiled data
    let inner_profiler = profiler.deref().as_ref().unwrap();
    let fname = &inner_profiler.fname;
    let timing = inner_profiler.timer.timing();
    let profile_data = inner_profiler
        .report
        .pprof(timing)
        .expect("pprof serialization");
    let data = profile_data
        .write_to_bytes()
        .expect("protobuf serialization");
    fs::write(fname, data).expect("write");

    // uninstall signal handler
    let handler = signal::SigHandler::SigIgn;
    unsafe { signal::signal(signal::SIGPROF, handler) }
        .map_err(|e| format!("sigaction uninstall error: {}", e))?;

    *profiler = None;

    Ok(())
}
