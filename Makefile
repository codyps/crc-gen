TARGET_BIN = crc-gen

ALL_CFLAGS += -I. -Iccan
obj-crc-gen = crc-gen.c.o printf-ext.c.o

all::

ccan/config.h: ccan/tools/configurator/configurator
	$^ > $@

ccan/tools/configurator/configurator : ccan/tools/configurator/configurator.c
	$(CC) $(ALL_CFLAGS) -o $@ $<

include base.mk
