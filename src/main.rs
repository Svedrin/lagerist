extern crate clap;
#[macro_use]
extern crate error_chain;
extern crate ctrlc;
extern crate libc;
#[macro_use]
extern crate prometheus;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::io::prelude::*;
use std::ffi::CString;
use std::net::TcpListener;
use std::os::unix::io::AsRawFd;
use prometheus::{TextEncoder, Encoder};

use clap::{Arg, App};

mod ktrace;
mod dev;

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

const HISTOGRAM_BUCKETS : [f64; 17] = [
    0.01,  0.025,  0.05,  0.075,
    0.1,   0.25,   0.5,   0.75,
    1.0,   2.5,    5.0,   7.5,
   10.0,  25.0,   50.0,  75.0,
  100.0
];


fn run(port: u16) -> Result<()> {
    // Initialize ^c handler
    let running = Arc::new(AtomicBool::new(true));
    let running_clone = running.clone();

    ctrlc::set_handler(move || {
        running_clone.store(false, Ordering::SeqCst);
    }).expect("Error setting Ctrl-C handler");

    // Set up Prometheus registry and histograms
    let h_queue_time = register_histogram_vec!(
        histogram_opts!("diskio_queue_time_seconds", "Time spent in the queue")
            .buckets(HISTOGRAM_BUCKETS.iter().map(|x| x / 1000.0).collect()),
        &["device", "optype"]
    ).expect("Couldn't set up queue time histogram");

    let h_disk_time = register_histogram_vec!(
        histogram_opts!("diskio_disk_time_seconds", "Time spent on the device")
            .buckets(HISTOGRAM_BUCKETS.iter().map(|x| x / 1000.0).collect()),
        &["device", "optype"]
    ).expect("Couldn't set up disk time histogram");

    let h_total_time = register_histogram_vec!(
        histogram_opts!("diskio_total_time_seconds", "Total time spent")
            .buckets(HISTOGRAM_BUCKETS.iter().map(|x| x / 1000.0).collect()),
        &["device", "optype"]
    ).expect("Couldn't set up total time histogram");

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

    let mut insertions = HashMap::new();
    let mut issuances = HashMap::new();
    let mut device_paths = dev::DevicePaths::new();

    while running.load(Ordering::SeqCst) {
        let poll_result = unsafe {
            libc::poll(
                &mut pollfds[0] as *mut libc::pollfd,
                pollfds.len() as u64,
                100
            )
        };
        if poll_result == -1 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::Interrupted {
                // Probably came from hitting ctrl+c. Just in case it didn't,
                // let the while loop make that decision.
                continue
            }
            else {
                bail!("Couldn't poll: {:?}", err);
            }
        }
        // Check for new data on the trace_pipe
        if pollfds[0].revents & libc::POLLIN != 0 {
            let data = {
                let bytes_read = unsafe {
                    libc::read(
                        trace_pipe_fd,
                        contents.as_mut_ptr() as *mut libc::c_void,
                        contents.len() - 1
                    ) as usize
                };
                String::from_utf8_lossy(&contents[..bytes_read])
            };
            for line in data.lines() {
                let words: Vec<&str> = line.split_ascii_whitespace().collect();
                // The definition seems to be from here:
                // https://github.com/torvalds/linux/blob/master/include/trace/events/block.h#L175
                // Looks like the fields in this TP_printk are words[5:]; words[0:4] seem to be constant.
                // Unfortunately, words[0] can contain whitespace. m(
                // Since we don't really need words[0], look for a subslice that starts at a position
                // such that words[1] contains the number in brackets.
                let mut start = None;
                for (idx, word) in words.iter().enumerate() {
                    if word.starts_with("[") {
                        start = Some(idx - 1);
                        break;
                    }
                }
                if start.is_none() {
                    eprintln!("Malformatted line (can't find '['): {}", line);
                    continue;
                }
                let words = &words[start.unwrap()..];
                //dbg!(words);
                // Get time from words[3]
                // Unfortunately there's a : at the end, so cut that away first
                let time = match words[3][..words[3].len() -1].parse::<f64>() {
                    Ok(t) => t,
                    Err(_) => {
                        eprintln!("Malformatted line (invalid time): {}", line);
                        continue;
                    }
                };

                // Op has the same : problem
                let op = &words[4][..words[4].len() - 1];
                let dev = words[5];
                let rwbs = words[6];

                if dev == "0,0" {
                    continue;
                }

                let optype =
                    if rwbs.contains("R") {
                        "read"
                    } else if rwbs.contains("W") {
                        "write"
                    } else {
                        //eprintln!("Ignoring line (unknown optype): {}", line);
                        continue;
                    };

                let dev_path = device_paths.get_dev_path(dev);

                match op {
                    "block_rq_insert" => {
                        // insert and issue ops have a request size field
                        let _reqsz = words[7];
                        let sector = words[9];
                        let nr_sectors = words[11];
                        let event_key = format!("{},{},{}", dev, sector, nr_sectors);
                        insertions.insert(event_key, time);
                    },
                    "block_rq_issue" => {
                        // insert and issue ops have a request size field
                        let _reqsz = words[7];
                        let sector = words[9];
                        let nr_sectors = words[11];
                        let event_key = format!("{},{},{}", dev, sector, nr_sectors);
                        issuances.insert(event_key, time);
                    },
                    "block_rq_complete" => {
                        // complete ops do not have the size field
                        let sector = words[8];
                        let nr_sectors = words[10];
                        let event_key = format!("{},{},{}", dev, sector, nr_sectors);
                        let insertion = match insertions.remove(&event_key) {
                            Some(t) => t,
                            None => continue
                        };
                        let issuance = match issuances.remove(&event_key) {
                            Some(t) => t,
                            None => continue
                        };
                        let queue_time = issuance - insertion;
                        let disk_time  = time - issuance;
                        let total_time = queue_time + disk_time;
                        dbg!(&dev_path, total_time);
                        h_queue_time.with_label_values(&[&dev_path, optype]).observe(queue_time);
                        h_disk_time.with_label_values(&[&dev_path, optype]).observe(disk_time);
                        h_total_time.with_label_values(&[&dev_path, optype]).observe(total_time);
                    },
                    _ => continue
                }
            }
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
                        client.read(&mut data)
                            .chain_err(|| "Could not read client data")?;

                        // We don't really care for the request, we always respond with our data
                        let mut buffer = Vec::new();
                        let encoder = TextEncoder::new();
                        let metric_families = prometheus::gather();
                        encoder.encode(&metric_families, &mut buffer).unwrap();
                        let output = String::from_utf8(buffer).unwrap();
                        let response = format!(
                            "HTTP/1.1 200 OK\nContent-Type: text/plain\nContent-Length: {}\n\n{}\n",
                            output.len(),
                            output
                        );
                        client.write(response.as_bytes()).chain_err(|| "Could not send response to client")?;
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

    unsafe {
        libc::close(trace_pipe_fd);
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
