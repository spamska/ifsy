# Nuke built-in rules and variables.
MAKEFLAGS += -rR
.SUFFIXES:

# This is the name that our final executable will have.
# Change as needed.
override OUTPUT := ifsy-kernel

# Convenience macro to reliably declare user overridable variables.
override USER_VARIABLE = $(if $(filter $(origin $(1)),default undefined),$(eval override $(1) := $(2)))

# Target architecture to build for. Default to x86_64.
$(call USER_VARIABLE,KARCH,x86_64)

# Supported architectures only: AMD64 (x86_64), ARM64 (aarch64) and RISC-V
# (riscv64). LoongArch is intentionally not supported.
override SUPPORTED_KARCHS := x86_64 aarch64 riscv64
ifeq ($(filter $(KARCH),$(SUPPORTED_KARCHS)),)
    $(error Unsupported KARCH '$(KARCH)'. Supported architectures are: $(SUPPORTED_KARCHS))
endif

ifeq ($(RUST_TARGET),)
    override RUST_TARGET := $(KARCH)-unknown-none
	ifeq ($(KARCH),riscv64)
    	override RUST_TARGET := riscv64gc-unknown-none-elf
	endif
endif

ifeq ($(RUST_PROFILE),)
    override RUST_PROFILE := release
endif

override RUST_PROFILE_SUBDIR := $(RUST_PROFILE)
ifeq ($(RUST_PROFILE),dev)
    override RUST_PROFILE_SUBDIR := debug
endif

# Default target.
.PHONY: all
all:
	# Position-independent kernel on every architecture so Limine can slide it (KASLR).
	# IFSY_PIE is read by the C crates' build.rs.
	# -pie is passed explicitly: rustc only adds it for targets whose spec says they support PIE,
	# and without it the ELF stays ET_EXEC with no PT_DYNAMIC, which Limine will not slide.
	# -z notext: the prebuilt core/alloc for the aarch64/riscv64 bare-metal targets are not PIC and keep
	# absolute pointers in read-only data; Limine applies those relocations at load time.
	IFSY_PIE=1 RUSTFLAGS="-C relocation-model=pic -C link-arg=-pie -C link-arg=-z -C link-arg=notext" cargo build --target $(RUST_TARGET) --profile $(RUST_PROFILE)
	cp target/$(RUST_TARGET)/$(RUST_PROFILE_SUBDIR)/$$(cd target/$(RUST_TARGET)/$(RUST_PROFILE_SUBDIR) && find -maxdepth 1 -perm -111 -type f) ifsy-kernel

# Remove object files and the final executable.
.PHONY: clean
clean:
	cargo clean
	rm -rf ifsy-kernel

.PHONY: distclean
distclean: clean
