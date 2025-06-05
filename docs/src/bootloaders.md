# Bootloaders in `bootc`

`bootc` uses [bootupd](https://github.com/coreos/bootupd/) by default to manage bootloader installation and configuration. `bootupd` is an external project that abstracts over bootloader installs and upgrades, providing a consistent interface for different bootloader types (e.g., GRUB, systemd-boot).

When you run `bootc install`, it invokes `bootupctl backend install` to install the bootloader to the target disk or filesystem. The specific bootloader configuration is determined by the container image and the target system's hardware.

Currently, `bootc` only runs `bootupd` during the installation process. It does **not** automatically run `bootupctl update` to update the bootloader after installation. This means that bootloader updates must be handled separately, typically by the user or an automated system update process.

For s390x, bootc uses `zipl` instead of `bootupd`.