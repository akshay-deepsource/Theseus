//! Creation and management of virtual consoles or terminals atop Theseus.

#![no_std]

extern crate alloc;
#[macro_use] extern crate log;
extern crate spin;
extern crate irq_safety;
extern crate bare_io;
extern crate mpmc;

extern crate task;
extern crate spawn;
extern crate async_channel;
extern crate serial_port;
extern crate io;
extern crate text_terminal;

use core::marker::PhantomData;

use alloc::string::String;
use task::TaskRef;
use async_channel::Receiver;
use serial_port::{SerialPort, SerialPortAddress, get_serial_port, DataChunk};
use io::LockableIo;
use text_terminal::{TerminalBackend, TextTerminal, TtyBackend};
use irq_safety::MutexIrqSafe;


/// Starts a new task that detects new console connections
/// by waiting for new data to be received on serial ports.
///
/// Returns the newly-spawned detection task.
pub fn start_connection_detection() -> Result<TaskRef, &'static str> {
	let (sender, receiver) = async_channel::new_channel(4);
	serial_port::set_connection_listener(sender);

	spawn::new_task_builder(console_connection_detector, receiver)
		.name("console_connection_detector".into())
		.spawn()
}

pub struct Console<I, O, Backend> 
	where I: bare_io::Read,
	      O: bare_io::Write,
		  Backend: TerminalBackend,
{
	name: String,
	_input: I,
	terminal: TextTerminal<Backend>,
	_output: PhantomData<O>,
}

/// Creates a new console and a new [`TextTerminal`] that reads input data 
/// from the given `input_stream`.
///
/// The terminal created by this function will use a [`TtyBackend`]
/// that writes terminal output and control commands to the given `output_stream`.
///
/// To start running the console, invoke the [`Console::spawn()`] function.
pub fn new_serial_console<S, I, O>(
	name: S,
	input_stream: I,
	output_stream: O,
) -> Console<I, O, TtyBackend<O>> 
	where S: Into<String>,
		  I: bare_io::Read,
	      O: bare_io::Write + Send + 'static,
{
	Console {
		name: name.into(),
		_input: input_stream,
		terminal: TextTerminal::new(80, 25, TtyBackend::new(None, output_stream)),
		_output: PhantomData,
	}
}


/// The entry point for the console connection detector task.
fn console_connection_detector(connection_listener: Receiver<SerialPortAddress>) -> Result<(), &'static str> {

	loop {
		let serial_port_address = connection_listener.receive().map_err(|e| {
			error!("Error receiving console connection request: {:?}", e);
			"error receiving console connection request"
		})?;
		let serial_port = match get_serial_port(serial_port_address) {
			Some(sp) => sp.clone(),
			_ => {
				warn!("Serial port {:?} was not initialized, skipping console connection request", serial_port_address);
				continue;
			}
		};
		
		let new_console = new_serial_console(
			alloc::format!("console_{:?}", serial_port_address),
			LockableIo::<_, MutexIrqSafe<SerialPort>, _>::from(serial_port.clone()),
			LockableIo::<_, MutexIrqSafe<SerialPort>, _>::from(serial_port.clone()),
		);
		
		let (sender, receiver) = async_channel::new_channel(16);
		serial_port.lock().set_data_sender(sender);

		let _taskref = spawn::new_task_builder(console_entry, (new_console, receiver))
			.name(alloc::format!("console_loop_{:?}", serial_port_address))
			.spawn()?;
	}

	// Err("console_connection_detector task returned unexpectedly")
}


/// The entry point for the each new [`Console`] task.
fn console_entry<I, O, Backend>(
	(mut console, input_receiver): (Console<I, O, Backend>, Receiver<DataChunk>),
) -> Result<(), &'static str> 
	where I: bare_io::Read,
	      O: bare_io::Write,
		  Backend: TerminalBackend,
{
	loop {
		// Block until we receive the next data chunk from the sender.
		match input_receiver.receive() {
			Ok(DataChunk { len, data }) => {
				let _res = console.terminal.handle_input(&mut &data[.. (len as usize)]);
			}
			Err(_e) => {
				error!("[LIKELY BUG] Error receiving input data on {:?}: {:?}. Retrying...",
					_e, console.name
				);
			}
		}
	}	

	// Err("console_entry task returned unexpectedly")
}
