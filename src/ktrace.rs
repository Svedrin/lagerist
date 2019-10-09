use std::fs::{File,create_dir,remove_dir,OpenOptions};
use std::io::prelude::*;
use super::errors::{Result, ResultExt};

fn echo_into(value: &[u8], path: &str) -> Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(&path)
        .chain_err(|| format!("could not open {}", &path))?;
    file.write_all(value)
        .chain_err(|| format!("could not write data to {}", &path))?;
    Ok(())
}


pub fn setup() -> Result<()> {
    // Basically, do the equivalent of:
    // INST="/sys/kernel/debug/tracing/instances/lagerist"
    // mkdir -p "$INST"
    // echo 1 > "$INST/events/block/block_rq_issue/enable"
    // echo 1 > "$INST/events/block/block_rq_insert/enable"
    // echo 1 > "$INST/events/block/block_rq_complete/enable"
    // echo 1 > "$INST/tracing_on"
    let instance_path = format!("/sys/kernel/debug/tracing/instances/{}", env!("CARGO_PKG_NAME"));
    create_dir(&instance_path)
        .or_else(
            |err| if err.kind() == std::io::ErrorKind::AlreadyExists {
                println!("ktrace instance already exists, using existing one");
                Ok(())
            } else {
                Err(err)
            }
        )
        .chain_err(|| "could not create ktrace instance")?;
    echo_into(b"1", &format!("{}/events/block/block_rq_issue/enable",    &instance_path))?;
    echo_into(b"1", &format!("{}/events/block/block_rq_insert/enable",   &instance_path))?;
    echo_into(b"1", &format!("{}/events/block/block_rq_complete/enable", &instance_path))?;
    echo_into(b"1", &format!("{}/tracing_on", &instance_path))?;
    Ok(())
}

pub fn teardown() -> Result<()> {
    // Basically, do the equivalent of:
    // INST="/sys/kernel/debug/tracing/instances/lagerist"
    // echo 0 > "$INST/tracing_on"
    // rmdir "$INST"
    let instance_path = format!("/sys/kernel/debug/tracing/instances/{}", env!("CARGO_PKG_NAME"));
    echo_into(b"0", &format!("{}/tracing_on", &instance_path))?;
    remove_dir(&instance_path)
        .chain_err(|| "could not remove ktrace instance")?;
    Ok(())
}
