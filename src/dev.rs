use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(60);

pub struct DevicePaths {
    cache: HashMap<String, (String, Instant)>,
}

impl DevicePaths {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
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

        let proc_partitions = fs::read_to_string("/proc/partitions")?;
        for line in proc_partitions.lines().skip(2) {
            let fields: Vec<&str> = line.split_ascii_whitespace().collect();
            if fields[0] == major && fields[1] == minor {
                name = Some(fields[3].clone());
            }
        }
        let name = match name {
            Some(n) => n,
            None => return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Device with no name: {}", dev),
            )),
        };
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
                        let placeholder = "\x00";
                        let escaped = entry_name.replace("--", placeholder);
                        let parts: Vec<&str> = escaped.splitn(2, "-").collect();
                        if parts.len() == 2 {
                            let vg = parts[0].replace(placeholder, "-");
                            let lv = parts[1].replace(placeholder, "-");
                            dev_path = format!("/dev/{}/{}", vg, lv);
                        }
                    } else {
                        dev_path = String::from(mapper_path.to_string_lossy());
                    }
                    break;
                }
            }
        }
        Ok(dev_path)
    }

    pub fn cleanup(&mut self) -> Vec<String> {
        let mut evicted = Vec::new();
        self.cache.retain(|_, (path, inserted_at)| {
            if inserted_at.elapsed() >= CACHE_TTL {
                evicted.push(path.clone());
                false
            } else {
                true
            }
        });
        evicted
    }


    pub fn get_dev_path(&mut self, dev: &str) -> (String, Option<String>) {
        // Dev is a "major,minor" string
        // Returns (current_path, evicted_old_path) where evicted_old_path is Some(old)
        // when a stale cache entry was replaced with a different path, so the caller can
        // clean up associated Prometheus label values for the old path.
        if let Some((path, inserted_at)) = self.cache.get(dev) {
            if inserted_at.elapsed() < CACHE_TTL {
                return (path.clone(), None);
            }
        }
        let old_path = self.cache.remove(dev).map(|(p, _)| p);
        let path = self.resolve(dev).unwrap_or_else(|_| format!("dev:{}", dev));
        self.cache.insert(String::from(dev), (path.clone(), Instant::now()));
        // Only signal eviction when the new path actually differs from the old one
        let evicted = old_path.filter(|old| old != &path);
        (path, evicted)
    }
}
