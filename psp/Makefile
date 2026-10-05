TARGET = usbnet
OBJS = usbnet.o net.o exports.o

USE_KERNEL_LIBS = 1
# As PSPLink builds its modules: the CFW resolves the SDK's kernel NIDs
# for a module that declares an old firmware version.
PSP_FW_VERSION = 271

CFLAGS = -O2 -G0 -Wall -fno-builtin-printf
# make TRACE=1: a diagnosis build that writes ms0:/usbnet-trace.txt
ifdef TRACE
CFLAGS += -DTRACE
endif
LIBS = -lpspusb_driver -lpspusbbus_driver -lpspsystemctrl_kernel

PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build_prx.mak
