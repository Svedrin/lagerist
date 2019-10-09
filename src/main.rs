extern crate clap;
#[macro_use]
extern crate error_chain;

use clap::{Arg, App};
use prometheus::{Opts, Registry, Counter, TextEncoder, Encoder};

mod ktrace;

mod errors {
    error_chain! { }
}

use errors::*;

fn run(port: u16) -> Result<()> {
    Ok(())
}

fn print_error(msg: &str, e: &Error) {
    eprintln!("{}: {}", msg, e);

    for e in e.iter().skip(1) {
        eprintln!("caused by: {}", e);
    }

    if let Some(backtrace) = e.backtrace() {
        eprintln!("backtrace: {:?}", backtrace);
    }
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
