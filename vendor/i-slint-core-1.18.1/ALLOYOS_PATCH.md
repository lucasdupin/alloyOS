# AlloyOS patch to i-slint-core 1.18.1

Source: the unmodified crates.io i-slint-core 1.18.1 distribution, with the changes below.

Slint's `software-renderer-path` feature enables a `f32::rem_euclid(1.0)` call
that does not compile for AlloyOS's `no_std` target. Replace that single call
with the equivalent remainder plus negative adjustment. The special handling
of nonzero integer endpoints remains unchanged; NaN and infinities retain
NaN behavior. This allows native Slint cubic vector paths in the guest.

Remove this patch when an upstream pinned Slint release supports this path in
`no_std`. Upstream copyright and license files are retained in this directory.

Host builds also allow unused compatibility imports on the five `num_traits::Float`
imports in animation and brush modules, and the `RemEuclid` fallback trait in
`graphics/image.rs`. Host floats provide these methods inherently; the traits
remain available for bare-metal builds. These allowances apply only to host
targets and do not disable warnings for AlloyOS code.
