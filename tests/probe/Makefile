# make: probe.prx, a user program that asks the device "usbnet:".
# make TARGET=alone: alone.prx, a kernel helper that takes PSPLink's USB away for a while.
TARGET ?= probe
ifeq ($(TARGET),alone)
OBJS = alone.o exports.o
USE_KERNEL_LIBS = 1
PSP_FW_VERSION = 271
CFLAGS = -O2 -G0 -Wall -fno-builtin-printf
LIBS = -lpspusb_driver
PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build_prx.mak
else
OBJS = main.o
BUILD_PRX = 1
CFLAGS = -O2 -G0 -Wall
PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build.mak
endif
