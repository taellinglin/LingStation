impl Drop for DawApp {
    fn drop(&mut self) {
        self.show_plugin_ui = false;
        self.destroy_plugin_ui();
        self.stop_audio_and_midi();
        self.leak_hosts_on_exit();
        self.startup_sink = None;
        self.startup_stream = None;
    }
}
