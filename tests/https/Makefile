# The HTTPS rounds need pspkit-https, as released: PSPKIT_HTTPS=<its unpacked folder>.
TARGET = httpsnight
OBJS = main.o

BUILD_PRX = 1
CFLAGS = -O2 -G0 -Wall
LIBS =

include $(PSPKIT_HTTPS)/module.mk
PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build.mak
