//! Home screen (stub — full implementation in Task 7).

use crate::bindings::lv_obj_t;
use std::sync::Arc;
use waved_state::{AudioState, MeetingState, StateBus};

pub struct HomeScreen;

impl HomeScreen {
    pub fn new(_bus: Arc<StateBus>, _connected_label: &str) -> Self { Self }
    pub unsafe fn create(&mut self) -> *mut lv_obj_t { std::ptr::null_mut() }
    pub unsafe fn update_state(&mut self, _audio: &AudioState, _meeting: &MeetingState) {}
}
