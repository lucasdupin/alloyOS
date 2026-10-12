pub mod automation;
pub mod block;
pub mod cobaltfs;
pub mod devfs;
pub mod ramfs;
pub mod vfs;

#[cfg(test)]
pub mod tests;

pub fn init() -> &'static str {
    vfs::init()
}
