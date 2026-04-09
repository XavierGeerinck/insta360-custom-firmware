pub mod bindings;
pub mod display;
pub mod i18n;
pub mod input;
pub mod nav;
pub mod screens;
pub mod theme;

pub struct UiService;

impl UiService {
    pub fn new() -> Self { Self }
}

#[cfg(test)]
mod tests {
    #[test]
    fn display_module_compiles() {
        assert!(true);
    }
}
