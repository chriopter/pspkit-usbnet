TARGET = return-xmb
OBJS = main.o exports.o
USE_KERNEL_LIBS = 1
PSP_FW_VERSION = 271
CFLAGS = -O2 -G0 -Wall -Wextra -fno-builtin-printf
LIBS = -lpspusb_driver -lpspusbbus_driver
PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build_prx.mak
