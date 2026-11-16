use std::collections::HashMap;
use std::fs;
use std::path::Path;

pub struct DevicePaths {
    cache: HashMap<String, String>,
    proc_partitions: String
}

impl DevicePaths {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            proc_partitions: fs::read_to_string("/proc/partitions")
                .expect("cannot read /proc/partitions")
        }
    }

    fn resolve(&self, dev: &str) -> std::io::Result<String> {
        let parts: Vec<&str> = dev.split(",").collect();
        let (major, minor) = (parts[0], parts[1]);
        let mut name = None;

        if major == "7" {
            // These are not listed in /proc/partitions but generate events
            return Ok(format!("/dev/loop{}", minor));
        }

        for line in self.proc_partitions.lines().skip(2) {
            let fields: Vec<&str> = line.split_ascii_whitespace().collect();
            if fields[0] == major && fields[1] == minor {
                name = Some(fields[3].clone());
            }
        }
        let name = name
            .expect(&format!("Device with no name: {}", dev));
        let mut dev_path = format!("/dev/{}", name);

        if name.starts_with("dm-") {
            for entry in fs::read_dir("/dev/mapper")? {
                let entry = entry?;
                let mapper_path = entry.path();
                let resolved = match fs::canonicalize(&mapper_path) {
                    Ok(p) => p,
                    Err(_) => continue,
                };
                if resolved == Path::new(&dev_path) {
                    let entry_name = String::from(entry.file_name().to_string_lossy());
                    if entry_name.contains("-") {
                        let parts: Vec<&str> = entry_name.splitn(2, "-").collect();
                        let vg = String::from(parts[0]);
                        let lv = String::from(parts[1]).replace("--", "-");
                        dev_path = format!("/dev/{}/{}", vg, lv);
                    } else {
                        dev_path = String::from(mapper_path.to_string_lossy());
                    }
                    break;
                }
            }
        }
        Ok(dev_path)
    }

    pub fn get_dev_path(&mut self, dev: &str) -> String {
        // Dev is a "major,minor" string
        if self.cache.contains_key(dev) {
            self.cache.get(dev).unwrap().clone()
        } else {
            let path = self.resolve(dev).expect("couldn't find name for device");
            self.cache.insert(String::from(dev), path.clone());
            path
        }
    }
}

