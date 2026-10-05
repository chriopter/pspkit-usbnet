TARGET = netup
OBJS = main.o

BUILD_PRX = 1
CFLAGS = -O2 -G0 -Wall
LIBS = -lpspwlan -lpsppower -lpspkubridge

PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build.mak
