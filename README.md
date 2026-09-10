# xmip-core-resilience-timeout

Timeout guard: an attempt that took longer than its limit is refused, even one that succeeded, unless the guard is lenient. A technology of [xmip-core-resilience](https://github.com/IlleNilsson/xmip-core-resilience).

## Toolchain

`rust-toolchain.toml` pins the toolchain for the whole estate. Do not change it
here.

## Verification

The included workflow is manual-only and calls the versioned shared workflow at
`IlleNilsson/.github@v1`.
