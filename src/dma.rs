#[derive(Clone, Copy, Default)]
pub(crate) struct Dma {
    pub(crate) source: u32,
    pub(crate) destination: u32,
    pub(crate) count: u16,
    pub(crate) control: u16,
}
