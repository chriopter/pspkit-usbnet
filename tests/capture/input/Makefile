TARGET = capture-input
OBJS = main.o exports.o
USE_KERNEL_LIBS = 1
PSP_FW_VERSION = 271
CFLAGS = -O2 -G0 -Wall -Wextra
LIBS = -lpspsystemctrl_kernel
PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build_prx.mak
