extern crate clap;
#[macro_use]
extern crate error_chain;
extern crate libc;

use std::io::prelude::*;
use std::ffi::CString;
use std::net::{TcpListener, TcpStream};
use std::os::unix::io::AsRawFd;

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

fn run(port: u16) -> Result<()> {
    /*let mut stream = File::open(ktrace::socket_path())
        .chain_err(|| "no open no stop mah show")?;*/
    let trace_pipe_fd = unsafe {
        libc::open(
            CString::new(ktrace::socket_path()).unwrap().as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK
        )
    };

    let listener = TcpListener::bind(format!(":::{}", port))
        .chain_err(|| "Could not start server")?;
    listener.set_nonblocking(true)
        .chain_err(|| "Could not set nonblocking")?;

    let mut clients = vec![];

    let mut pollfds = vec![
        libc::pollfd {
            //fd:      stream.as_raw_fd(),
            fd:      trace_pipe_fd,
            events:  libc::POLLIN,
            revents: 0
        },
        libc::pollfd {
            fd:      listener.as_raw_fd(),
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
        // Check for new data on the trace_pipe
        if pollfds[0].revents & libc::POLLIN != 0 {
            let bytes_read = unsafe {
                libc::read(
                    trace_pipe_fd,
                    contents.as_mut_ptr() as *mut libc::c_void,
                    contents.len() - 1
                )
            } as usize;
            let data = String::from_utf8_lossy(&contents[..bytes_read]);
            dbg!(data);
        }
        // Check for a new incoming connection on the listener
        if pollfds[1].revents & libc::POLLIN != 0 {
            for stream in listener.incoming() {
                match stream {
                    Ok(stream) => {
                        if let Ok(addr) = stream.peer_addr() {
                            dbg!(addr);
                        }

                        pollfds.push(
                            libc::pollfd {
                                fd: stream.as_raw_fd(),
                                events: libc::POLLIN,
                                revents: 0
                            }
                        );
                        clients.push(stream);
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        break;
                    }
                    Err(e) => {
                        Err(e).chain_err(|| "Could not accept client connection")?;
                    }
                }
            }
        }
        // Check any additional fds as client connections
        let mut remove_client = None;
        for (client_fd_idx, client_fd) in pollfds.iter().enumerate().skip(2) {
            if client_fd.revents & libc::POLLIN != 0 {
                // Find the TcpStream that this fd belongs to
                for (client_idx, client) in clients.iter_mut().enumerate() {
                    if client.as_raw_fd() == client_fd.fd {
                        // Handle the request. We always use Connection:Close semantics because easier.
                        remove_client = Some((client_fd_idx, client_idx));
                        let mut data = [0u8; 16_384];
                        let data_len = client.read(&mut data)
                            .chain_err(|| "Could not read client data")?
                            as usize;
                        // We don't really care for the request, we always respond with our data
                        client.write(
                            b"HTTP/1.1 200 OK\nContent-Type: text/plain\nContent-Length: 6\n\nhallo\n"
                        ).chain_err(|| "Could not send response to client")?;
                        break;
                    }
                }
            }
        }
        if let Some((client_fd_idx, client_idx)) = remove_client.take() {
            pollfds.remove(client_fd_idx);
            clients.remove(client_idx);
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
