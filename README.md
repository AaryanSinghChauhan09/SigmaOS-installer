# SigmaOS-installer

Installer component for SigmaOS — installation state machine for the partition/copy/bootloader flow. Disk operations are future work.

- **Category:** Utilities
- **Engine:** installer
- **Language:** Rust

## Build & Test

```sh
cargo build
cargo test
```

Part of the SigmaOS multi-repository component ecosystem.
## Documentation

Wiki documentation is available in the SigmaOS Wiki Staging repository:

- [SigmaOS Wiki Staging](https://github.com/AaryanSinghChauhan09/sigmaos-wiki-staging#SigmaOS-installer) - Full Wiki documentation for all SigmaOS components

## See Also

Related SigmaOS **Core System** repositories:

- [SigmaOS-sigma-kernel](../SigmaOS-sigma-kernel/README.md) — SigmaOS microkernel / x86_64 kernel
- [SigmaOS-sigma-sh](../SigmaOS-sigma-sh/README.md) — SigmaOS interactive userspace shell
- [SigmaOS-sigma-sys](../SigmaOS-sigma-sys/README.md) — SigmaOS syscall ABI & userspace interface
- [SigmaOS-initramfs](../SigmaOS-initramfs/README.md) — SigmaOS initramfs builder & hook planner
- [SigmaOS-sigma-bootloader](../SigmaOS-sigma-bootloader/README.md) — SigmaOS bootloader
- [SigmaOS-ISO](../SigmaOS-ISO/README.md) — SigmaOS live & installable ISO builder
