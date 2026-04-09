pub mod bindings;
pub mod display;
pub mod input;

pub struct UiService;

impl UiService {
    pub fn new() -> Self { Self }
}

#[cfg(test)]
mod tests {
    #[test]
    fn display_module_compiles() {
        // Compile-check: if display.rs compiles, this passes.
        assert!(true);
    }
}
