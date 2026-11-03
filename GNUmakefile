# User space (rc and the core utilities) is built by user/Makefile and embedded
# into the kernel, which is built by sys/GNUmakefile (cargo; its build.rs builds user/ first). This file adds the
# boot-image, firmware and QEMU targets around it.
#
#   make                      build the bootable ISO
#   make run                  build an ISO and boot it in QEMU (UEFI)
#   make run KARCH=aarch64    the same for aarch64 or riscv64
#   make all-hdd              build a raw disk image

SHELL := /bin/bash
MAKEFLAGS += --no-print-directory

HOST_ARCH := $(shell uname -m | sed -e s/arm64/aarch64/ -e s/amd64/x86_64/)
KARCH ?= $(HOST_ARCH)
export KARCH

ifeq ($(filter $(KARCH),x86_64 aarch64 riscv64),)
$(error Unsupported KARCH '$(KARCH)'. Supported architectures are: x86_64 aarch64 riscv64)
endif

MKDISK   ?= y
DISKSIZE ?= 16

KERNEL := sys/ifsy-kernel
ISO    := ifsy-$(KARCH).iso
HDD    := ifsy-$(KARCH).hdd

# Hardware acceleration: only when the target matches the host and /dev/kvm is usable.
KVM = $(shell if [ "$(KARCH)" = "$(HOST_ARCH)" ] && [ -z "$(NOKVM)" ]; then \
	if [ -w /dev/kvm ]; then echo -enable-kvm; \
	else echo "note: /dev/kvm is not usable; running $(KARCH) without KVM" >&2; fi; fi)

# QEMU configuration for UEFI boots.
ifeq ($(KARCH),x86_64)
QEMU_MACHINE := q35
QEMU_X :=
QEMU_NIC := -netdev user,id=n0 -device virtio-net-pci,netdev=n0,disable-modern=on
else
QEMU_MACHINE := virt
QEMU_X := -device ramfb -drive if=pflash,unit=1,format=raw,file=edk2-ovmf/ovmf-vars-$(KARCH).fd -device virtio-keyboard-device
QEMU_NIC := -netdev user,id=n0 -device virtio-net-device,netdev=n0
endif
ifneq ($(KARCH),riscv64)
QEMU_X := -cpu max $(QEMU_X)
endif

QEMU_UEFI = qemu-system-$(KARCH) -M $(QEMU_MACHINE) $(QEMU_X) $(QEMU_NIC) \
	-drive if=pflash,unit=0,format=raw,file=edk2-ovmf/ovmf-code-$(KARCH).fd,readonly=on \
	-serial stdio -m 2G $(KVM) $(QEMUFLAGS)

QEMU_BIOS = qemu-system-$(KARCH) -M q35 -cpu max -netdev user,id=n0 \
	-device virtio-net-pci,netdev=n0,disable-modern=on -serial stdio $(KVM) \
	$(or $(QEMUFLAGS),-m 2G)

.PHONY: all all-hdd run run-hdd run-bios run-hdd-bios
.PHONY: ovmf ovmf-x86_64 ovmf-aarch64 ovmf-riscv64 limine user kernel ramdisk
.PHONY: iso iso-pack iso-x86_64 iso-aarch64 iso-riscv64
.PHONY: hdd hdd-pack hdd-x86_64 hdd-aarch64 hdd-riscv64
.PHONY: clean distclean

all: iso

all-hdd: hdd

run: iso ovmf
	$(QEMU_UEFI) -cdrom $(ISO)

run-hdd: hdd ovmf
	$(QEMU_UEFI) -hda $(HDD)

run-bios: iso
	$(QEMU_BIOS) -cdrom $(ISO) -boot d

run-hdd-bios: hdd
	$(QEMU_BIOS) -hda $(HDD)

ovmf:
	mkdir -p edk2-ovmf
	$(MAKE) ovmf-$(KARCH)

ovmf-x86_64:
	[ -f edk2-ovmf/ovmf-code-x86_64.fd ] || curl -fL --max-time 600 -o edk2-ovmf/ovmf-code-x86_64.fd https://qemu.weilnetz.de/test/ovmf/usr/share/OVMF/OVMF_CODE_4M.fd
	[ -f edk2-ovmf/ovmf-vars-x86_64.fd ] || curl -fL --max-time 600 -o edk2-ovmf/ovmf-vars-x86_64.fd https://qemu.weilnetz.de/test/ovmf/usr/share/OVMF/OVMF_VARS_4M.fd
	[ -f edk2-ovmf/ovmf-secure-code-x86_64.fd ] || { curl -fL --max-time 600 -o edk2-ovmf/ovmf-secure-code-x86_64.fd.bz2 https://gitlab.com/qemu-project/qemu/-/raw/master/pc-bios/edk2-x86_64-secure-code.fd.bz2 && bunzip2 -f edk2-ovmf/ovmf-secure-code-x86_64.fd.bz2; }
	[ -f edk2-ovmf/ovmf-microvm-x86_64.fd ] || { curl -fL --max-time 600 -o edk2-ovmf/ovmf-microvm-x86_64.fd.bz2 https://gitlab.com/qemu-project/qemu/-/raw/master/pc-bios/edk2-x86_64-microvm.fd.bz2 && bunzip2 -f edk2-ovmf/ovmf-microvm-x86_64.fd.bz2; }

ovmf-aarch64:
	[ -f edk2-ovmf/ovmf-code-aarch64.fd ] || { curl -fL --max-time 600 -o edk2-ovmf/ovmf-code-aarch64.fd.bz2 https://gitlab.com/qemu-project/qemu/-/raw/master/pc-bios/edk2-aarch64-code.fd.bz2 && bunzip2 -f edk2-ovmf/ovmf-code-aarch64.fd.bz2; }
	[ -f edk2-ovmf/ovmf-vars-aarch64.fd ] || { curl -fL --max-time 600 -o edk2-ovmf/ovmf-vars-aarch64.fd.bz2 https://gitlab.com/qemu-project/qemu/-/raw/master/pc-bios/edk2-aarch64-vars.fd.bz2 && bunzip2 -f edk2-ovmf/ovmf-vars-aarch64.fd.bz2; }

ovmf-riscv64:
	[ -f edk2-ovmf/ovmf-code-riscv64.fd ] || { curl -fL --max-time 600 -o edk2-ovmf/ovmf-code-riscv64.fd.bz2 https://gitlab.com/qemu-project/qemu/-/raw/master/pc-bios/edk2-riscv64-code.fd.bz2 && bunzip2 -f edk2-ovmf/ovmf-code-riscv64.fd.bz2; }
	[ -f edk2-ovmf/ovmf-vars-riscv64.fd ] || { curl -fL --max-time 600 -o edk2-ovmf/ovmf-vars-riscv64.fd.bz2 https://gitlab.com/qemu-project/qemu/-/raw/master/pc-bios/edk2-riscv64-vars.fd.bz2 && bunzip2 -f edk2-ovmf/ovmf-vars-riscv64.fd.bz2; }

limine:
	rm -rf limine
	curl -s https://api.github.com/repos/Limine-Bootloader/Limine/releases | jq -r '.[] | select(.tag_name | startswith("v12.")) | .assets[].browser_download_url | select(endswith("limine-binary.tar.gz"))' | head -1 | xargs curl -L -o limine-binary.tar.gz
	tar -xzf limine-binary.tar.gz
	mv limine-binary limine
	rm limine-binary.tar.gz
	$(MAKE) -C limine limine

# Fetch and build Limine only when it is not there yet.
limine/limine:
	$(MAKE) limine

# rc and the core utilities. The kernel's build.rs runs this itself; this target is for building them alone.
user:
	$(MAKE) -C user ARCH=$(KARCH)

kernel:
	$(MAKE) -C sys KARCH=$(KARCH)

# Boot module handed to the kernel by Limine (see limine.conf).
ramdisk:
	@[ -f ext4.img ] || { \
	case "$(MKDISK)" in \
	[Yy]*) \
		case "$(DISKSIZE)" in ''|0|*[!0-9]*) echo "Invalid disk size: $(DISKSIZE)" >&2; exit 1;; esac; \
		dd if=/dev/zero of=ext4.img bs=1M count=$(DISKSIZE) && \
		{ mkfs.ext4 -q -F -O ^metadata_csum,^64bit,^orphan_file -L ifsy ext4.img 2>/dev/null || \
		  mkfs.ext4 -q -F -O ^metadata_csum,^64bit -L ifsy ext4.img; };; \
	esac; }

iso: limine/limine kernel ramdisk
	$(MAKE) iso-pack

iso-pack:
	rm -rf iso_root
	mkdir -p iso_root/boot
	cp -v $(KERNEL) iso_root/boot/
	cp -v ext4.img iso_root/boot/
	mkdir -p iso_root/boot/limine
	cp -v limine.conf iso_root/boot/limine/
	mkdir -p iso_root/EFI/BOOT
	$(MAKE) iso-$(KARCH)
	rm -rf iso_root

iso-x86_64:
	cp -v limine/limine-bios.sys limine/limine-bios-cd.bin limine/limine-uefi-cd.bin iso_root/boot/limine/
	cp -v limine/BOOTX64.EFI iso_root/EFI/BOOT/
	cp -v limine/BOOTIA32.EFI iso_root/EFI/BOOT/
	xorriso -as mkisofs -b boot/limine/limine-bios-cd.bin -no-emul-boot -boot-load-size 4 -boot-info-table --efi-boot boot/limine/limine-uefi-cd.bin -efi-boot-part --efi-boot-image --protective-msdos-label iso_root -o ifsy-x86_64.iso
	./limine/limine bios-install ifsy-x86_64.iso

iso-aarch64:
	cp -v limine/limine-uefi-cd.bin iso_root/boot/limine/
	cp -v limine/BOOTAA64.EFI iso_root/EFI/BOOT/
	xorriso -as mkisofs --efi-boot boot/limine/limine-uefi-cd.bin -efi-boot-part --efi-boot-image --protective-msdos-label iso_root -o ifsy-aarch64.iso

iso-riscv64:
	cp -v limine/limine-uefi-cd.bin iso_root/boot/limine/
	cp -v limine/BOOTRISCV64.EFI iso_root/EFI/BOOT/
	xorriso -as mkisofs --efi-boot boot/limine/limine-uefi-cd.bin -efi-boot-part --efi-boot-image --protective-msdos-label iso_root -o ifsy-riscv64.iso


hdd: limine/limine kernel ramdisk
	$(MAKE) hdd-pack

hdd-pack:
	rm -f $(HDD)
	dd if=/dev/zero bs=1M count=0 seek=64 of=$(HDD)
	sgdisk $(HDD) -n 1:2048 -t 1:ef00 -m 1
	if [ "$(KARCH)" = x86_64 ]; then ./limine/limine bios-install $(HDD); fi
	mformat -i $(HDD)@@1M
	mmd -i $(HDD)@@1M ::/EFI ::/EFI/BOOT ::/boot ::/boot/limine
	mcopy -i $(HDD)@@1M $(KERNEL) ::/boot
	mcopy -i $(HDD)@@1M limine.conf ::/boot/limine
	mcopy -i $(HDD)@@1M ext4.img ::/boot
	$(MAKE) hdd-$(KARCH)

hdd-x86_64:
	mcopy -i ifsy-x86_64.hdd@@1M limine/limine-bios.sys ::/boot/limine
	mcopy -i ifsy-x86_64.hdd@@1M limine/BOOTX64.EFI ::/EFI/BOOT
	mcopy -i ifsy-x86_64.hdd@@1M limine/BOOTIA32.EFI ::/EFI/BOOT

hdd-aarch64:
	mcopy -i ifsy-aarch64.hdd@@1M limine/BOOTAA64.EFI ::/EFI/BOOT

hdd-riscv64:
	mcopy -i ifsy-riscv64.hdd@@1M limine/BOOTRISCV64.EFI ::/EFI/BOOT

clean:
	cd sys && cargo clean
	$(MAKE) -C user clean
	rm -rf iso_root edk2-ovmf/ $(KERNEL) ext4.img
	rm -rf ifsy-x86_64.iso ifsy-x86_64.hdd
	rm -rf ifsy-aarch64.iso ifsy-aarch64.hdd
	rm -rf ifsy-riscv64.iso ifsy-riscv64.hdd

distclean: clean
	$(MAKE) -C sys distclean
	rm -rf limine ovmf
