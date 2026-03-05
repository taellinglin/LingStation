#[derive(Default)]
pub struct VstHost {
    pub plugin_paths: Vec<String>,
}

impl VstHost {
    pub fn new() -> Self {
        Self {
            plugin_paths: Vec::new(),
        }
    }

    pub fn load_plugin(&mut self, path: &str) -> Result<(), String> {
        if path.is_empty() {
            return Err("plugin path is empty".to_string());
        }
        self.plugin_paths.push(path.to_string());
        Ok(())
    }
}
