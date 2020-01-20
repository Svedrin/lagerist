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
            for path in fs::read_dir("/dev/mapper")? {
                let link_target = fs::read_link(&path?.path().as_path())?;
                if link_target.as_path() == Path::new(&dev_path) {
                    let file_name = String::from(link_target.file_name().unwrap().to_string_lossy());
                    if file_name.contains("-") {
                        let parts: Vec<&str> = file_name.splitn(2, "-").collect();
                        let vg = String::from(parts[0]);
                        let lv = String::from(parts[1]).replace("--", "-");
                        dev_path = format!("/dev/{}/{}", vg, lv);
                    } else {
                        dev_path = String::from(link_target.to_string_lossy());
                    }
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

