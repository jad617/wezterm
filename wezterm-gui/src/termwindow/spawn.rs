use crate::spawn::SpawnWhere;
use config::keyassignment::{SpawnCommand, SpawnTabDomain};
use config::TermConfig;
use mux::Mux;
use std::sync::Arc;

impl super::TermWindow {
    pub fn spawn_command(&self, spawn: &SpawnCommand, spawn_where: SpawnWhere) {
        // Splitting happens asynchronously, by which point the leader key
        // assignment that decided whether to split the focused float or
        // the pane underneath it is over, so resolve the pane now
        let spawn_where = match spawn_where {
            SpawnWhere::SplitPane(request) => match self.get_target_tab() {
                Some(tab) if Mux::get().get_float_parent(tab.tab_id()).is_some() => {
                    match tab.get_active_pane() {
                        Some(pane) => SpawnWhere::SplitPaneOf(pane.pane_id(), request),
                        None => return,
                    }
                }
                _ => spawn_where,
            },
            _ => spawn_where,
        };
        let size = if spawn_where == SpawnWhere::NewWindow {
            self.config.initial_size(
                self.dimensions.dpi as u32,
                crate::cell_pixel_dims(&self.config, self.dimensions.dpi as f64).ok(),
            )
        } else {
            self.terminal_size
        };
        let term_config = Arc::new(TermConfig::with_config(self.config.clone()));

        crate::spawn::spawn_command_impl(
            spawn,
            spawn_where,
            size,
            Some(self.mux_window_id),
            term_config,
        )
    }

    pub fn spawn_tab(&mut self, domain: &SpawnTabDomain) {
        self.spawn_command(
            &SpawnCommand {
                domain: domain.clone(),
                ..Default::default()
            },
            SpawnWhere::NewTab,
        );
    }
}
