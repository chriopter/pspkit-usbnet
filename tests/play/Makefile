# For tests of a game without hands: launch.prx starts an ISO from PSPLink, pad.prx is a
# plugin that presses buttons by a timetable and photographs the screen.
#   make TARGET=launch && make TARGET=pad && make TARGET=awake   (awake.prx keeps the PSP from sleeping)
TARGET ?= launch
OBJS = $(TARGET).o exports.o

USE_KERNEL_LIBS = 1
PSP_FW_VERSION = 271

CFLAGS = -O2 -G0 -Wall -fno-builtin-printf
LIBS = -lpspsystemctrl_kernel -lpspusb_driver -lpsppower_driver

PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build_prx.mak
