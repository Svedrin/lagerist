extern crate clap;
#[macro_use]
extern crate error_chain;
extern crate libc;

use std::ffi::CString;
//use std::os::unix::io::AsRawFd;

use clap::{Arg, App};

mod ktrace;

mod errors {
    error_chain! { }
}

use errors::*;

fn print_error(msg: &str, e: &Error) {
    eprintln!("{}: {}", msg, e);

    for e in e.iter().skip(1) {
        eprintln!("caused by: {}", e);
    }

    if let Some(backtrace) = e.backtrace() {
        eprintln!("backtrace: {:?}", backtrace);
    }
}

fn run(_port: u16) -> Result<()> {
    /*let mut stream = File::open(ktrace::socket_path())
        .chain_err(|| "no open no stop mah show")?;*/
    let trace_pipe_fd = unsafe {
        libc::open(
            CString::new(ktrace::socket_path()).unwrap().as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK
        )
    };
    let mut pollfds = [
        libc::pollfd {
            //fd:      stream.as_raw_fd(),
            fd:      trace_pipe_fd,
            events:  libc::POLLIN,
            revents: 0
        }
    ];
    let mut contents = vec![0u8; 10 * 1024 * 1024];
    loop {
        let poll_result = unsafe {
            libc::poll(
                &mut pollfds[0] as *mut libc::pollfd,
                pollfds.len() as u64,
                100
            )
        };
        if poll_result == -1 {
            bail!("Couldn't poll: {:?}", std::io::Error::last_os_error());
        }
        if pollfds[0].revents & libc::POLLIN != 0 {
            println!("READ OMFG");
            let bytes_read = unsafe {
                libc::read(
                    trace_pipe_fd,
                    contents.as_mut_ptr() as *mut libc::c_void,
                    contents.len() - 1
                )
            } as usize;
            println!("Dataz: {}", String::from_utf8_lossy(&contents[..bytes_read]));
        }
    }

    Ok(())
}

fn main() {
    let matches = App::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .author("Michael Ziegler <diese-addy@funzt-halt.net>")
        .about("disk IO metrics exporter")
        .arg(Arg::with_name("port")
            .short("p")
            .long("port")
            .takes_value(true)
            .help("Port number to use")
            .default_value("9165")
        )
        .get_matches();

    let port = match matches.value_of("port").unwrap().parse::<u16>() {
        Err(_) => {
            eprintln!("Port argument must be a number between 1 and 65535");
            ::std::process::exit(2);
        }
        Ok(port) => port
    };

    if let Err(err) = ktrace::setup() {
        print_error("Could not set up ktrace", &err);
        ::std::process::exit(1);
    }

    let returncode =
        if let Err(err) = run(port) {
            print_error("error", &err);
            1
        } else {
            0
        };

    if let Err(err) = ktrace::teardown() {
        print_error("Could not tear down ktrace", &err);
        eprintln!(
            "You'll probably want to rmdir /sys/kernel/debug/tracing/instances/{}",
            env!("CARGO_PKG_NAME")
        );
        ::std::process::exit(1);
    }

    ::std::process::exit(returncode);
}
