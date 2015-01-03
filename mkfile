CC=cc
CFLAGS=$CFLAGS -I. -Iccan -std=gnu99

crc-gen : crc-gen.c.o printf-ext.c.o
	$CC $CFLAGS -o $target $prereq

%.c.o : %.c ccan/config.h
	$CC $CFLAGS -c $stem.c -o $target

ccan/config.h: ccan/tools/configurator/configurator
	$prereq > $target

ccan/tools/configurator/configurator : ccan/tools/configurator/configurator.c
	$CC $CFLAGS -o $target $prereq

clean:V:
	rm -rf *.o crc-gen ccan/config.h ccan/tools/configurator/configurator
