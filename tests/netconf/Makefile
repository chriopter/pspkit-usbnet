TARGET = netconf-test
OBJS = main.o
BUILD_PRX = 1
CFLAGS = -O2 -G0 -Wall -Wextra
LIBS = -lpspgu
PSPSDK = $(shell psp-config --pspsdk-path)
include $(PSPSDK)/lib/build.mak
