// Pure check of the SSID-change detector used by the poll loop.
use waved_wifi::ssid_changed;

#[test]
fn same_ssid_no_event() {
    assert!(!ssid_changed(Some("A"), "A"));
}

#[test]
fn different_ssid_fires() {
    assert!(ssid_changed(Some("A"), "B"));
}

#[test]
fn first_connect_fires() {
    assert!(ssid_changed(None, "A"));
}
