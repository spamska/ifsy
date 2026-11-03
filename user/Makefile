# User space for Ifsy: the 9front libc, rc and the core utilities, built with the host gcc as
# static-PIE ELF64 programs for the kernel's P9 system call ABI.
#
# Architecture-specific pieces live in arch/$(ARCH)/ (crt0.S, gensys.sh).

# make ARCH=riscv64 cross-builds the riscv64 userland (needs a riscv64 gcc,
# e.g. gcc-riscv64-linux-gnu; override with CROSS=). The x86_64 build keeps
# using $(B)/ and bin/x86_64/ exactly as before.
ARCH	= x86_64
BIN	= bin/$(ARCH)
B	= build$(if $(filter-out x86_64,$(ARCH)),-$(ARCH))

ifeq ($(ARCH),riscv64)
CROSS	?= riscv64-linux-gnu-
# rv64gc/lp64d like the kernel; no linker relaxation, so no gp setup is needed.
# -fvisibility=hidden: bind symbols locally, so the static-PIE has only R_RISCV_RELATIVE
# relocations (the kernel's loader does not resolve symbols; GNU ld otherwise emits
# R_RISCV_64 for the address of a global, e.g. rc's table of Xfoo functions).
ARCHFLAGS = -march=rv64gc -mabi=lp64d -mno-relax -fvisibility=hidden
LDARCH = -Wl,-Bsymbolic
else ifeq ($(ARCH),aarch64)
CROSS	?= aarch64-linux-gnu-
ARCHFLAGS = -fvisibility=hidden -mno-outline-atomics
LDARCH = -Wl,-Bsymbolic
else
CROSS	?=
ARCHFLAGS =
LDARCH =
endif

CC	= $(CROSS)gcc
AR	= $(CROSS)ar
CFLAGS	= -nostdinc -isystem $(shell $(CC) -print-file-name=include) -O2 -fPIE -ffreestanding -fno-builtin -fno-stack-protector \
	  -fno-tree-loop-distribute-patterns -fno-asynchronous-unwind-tables \
	  -fplan9-extensions -w -DPlan9 -Iinclude -I$(B) -Irc $(ARCHFLAGS)
LDFLAGS	= -static-pie -nostdlib -Wl,--no-dynamic-linker -Wl,-z,norelro \
	  -Wl,-z,separate-code -Wl,-z,max-page-size=4096 -Wl,--build-id=none -Wl,-z,noexecstack $(ARCHFLAGS) $(LDARCH)

LIBSRC	= $(wildcard lib/bio/*.c) $(wildcard lib/regexp/*.c) $(wildcard lib/fmt/*.c) $(wildcard lib/port/*.c) $(wildcard lib/9sys/*.c) lib/glue.c lib/util.c lib/string.c
LIBOBJ	= $(patsubst lib/%.c,$(B)/lib/%.o,$(LIBSRC)) $(B)/lib/sys.o $(B)/lib/setjmp.o $(B)/lib/tas.o $(B)/lib/crt0.o
RCSRC	= $(filter-out rc/plan9.c,$(wildcard rc/*.c)) rc/plan9.c
RCOBJ	= $(patsubst rc/%.c,$(B)/rc/%.o,$(RCSRC)) $(B)/rc/y.tab.o
CMDS	= echo cat ls pwd mkdir rm touch cp mv ln uname date sleep kill wc head tail sort du df whoami
CMDBIN	= $(patsubst %,$(BIN)/%,$(CMDS))

all: $(BIN)/rc $(CMDBIN)

$(B)/y.tab.c $(B)/y.tab.h: rc/syn.y
	@mkdir -p $(B)
	bison -y -d rc/syn.y -o $(B)/y.tab.c 2>/dev/null
	cp $(B)/y.tab.h $(B)/x.tab.h

$(B)/lib/sys.S: lib/sys.h arch/$(ARCH)/gensys.sh
	@mkdir -p $(@D)
	sh arch/$(ARCH)/gensys.sh lib/sys.h > $@

$(B)/lib/sys.o: $(B)/lib/sys.S
	$(CC) $(ARCHFLAGS) -c -o $@ $<

$(B)/lib/setjmp.o: arch/$(ARCH)/setjmp.S
	@mkdir -p $(@D)
	$(CC) $(ARCHFLAGS) -c -o $@ $<

$(B)/lib/tas.o: arch/$(ARCH)/tas.S
	@mkdir -p $(@D)
	$(CC) $(ARCHFLAGS) -c -o $@ $<

$(B)/lib/crt0.o: arch/$(ARCH)/crt0.S
	@mkdir -p $(@D)
	$(CC) $(ARCHFLAGS) -c -o $@ $<

$(B)/lib/%.o: lib/%.c
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -c -o $@ $<

$(B)/rc/y.tab.o: $(B)/y.tab.c
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -c -o $@ $<

$(B)/rc/%.o: rc/%.c $(B)/y.tab.h
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -c -o $@ $<

$(B)/libc.a: $(LIBOBJ)
	rm -f $@
	$(AR) rcs $@ $(filter-out $(B)/lib/crt0.o,$^)

$(BIN)/rc: $(RCOBJ) $(B)/libc.a $(B)/lib/crt0.o
	@mkdir -p $(BIN)
	$(CC) $(LDFLAGS) -o $@ $(B)/lib/crt0.o $(RCOBJ) $(B)/libc.a

$(B)/cmd/%.o: cmd/%.c
	@mkdir -p $(@D)
	$(CC) $(CFLAGS) -c -o $@ $<

$(BIN)/%: $(B)/cmd/%.o $(B)/libc.a $(B)/lib/crt0.o
	@mkdir -p $(BIN)
	$(CC) $(LDFLAGS) -o $@ $(B)/lib/crt0.o $< $(B)/libc.a

clean:
	rm -rf build build-* bin
.PHONY: all clean
