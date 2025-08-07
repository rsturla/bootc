#!/bin/bash
installkernel() {
    instmods erofs overlay
}
check() {
    require_binaries /usr/lib/bootc/initramfs-setup || return 1
}
depends() {
    return 0
}
install() {
    local service=bootc-root-setup.service
    dracut_install /usr/lib/bootc/initramfs-setup
    inst_simple "${systemdsystemunitdir}/${service}"
    mkdir -p "${initdir}${systemdsystemconfdir}/initrd-root-fs.target.wants"
    ln_r "${systemdsystemunitdir}/${service}" \
        "${systemdsystemconfdir}/initrd-root-fs.target.wants/${service}"
}
