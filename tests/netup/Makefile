TARGET = netup
OBJS = main.o scan.o

BUILD_PRX = 1
CFLAGS = -O2 -G0 -Wall
ASFLAGS = $(CFLAGS)
LIBS = -lpspwlan -lpsppower -lpspkubridge

PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build.mak
