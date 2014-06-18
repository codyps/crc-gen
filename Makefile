TARGETS = crc-gen

ALL_CFLAGS += -I.
obj-crc-gen = crc-gen.o printf-ext.o

include base.mk
