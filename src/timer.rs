#[derive(Clone, Copy, Default)]
pub(crate) struct Timer {
    pub(crate) counter: u16,
    pub(crate) reload: u16,
    pub(crate) control: u16,
    pub(crate) prescaler: u32,
}
