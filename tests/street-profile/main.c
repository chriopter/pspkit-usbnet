/* Inspect saved profiles; pass "create" to add Hi-Speed USB in a free slot.
 * Does not initialize the network stack or call ExitGame. */
#include <pspkernel.h>
#include <psputility.h>
#include <stdarg.h>
#include <stdio.h>
#include <string.h>
PSP_MODULE_INFO("street_profile", 0, 1, 0);
PSP_MAIN_THREAD_ATTR(THREAD_ATTR_USER);
PSP_MAIN_THREAD_STACK_SIZE_KB(32);
PSP_HEAP_SIZE_KB(128);
static int logfd = -1;
static void say(const char *fmt, ...) {
    char line[256]; va_list ap;
    va_start(ap,fmt); int n=vsnprintf(line,sizeof line,fmt,ap); va_end(ap);
    if(n>0 && n<(int)sizeof line) sceIoWrite(logfd,line,n);
}
static void finish(void) __attribute__((noreturn));
static void finish(void) {
    if(logfd>=0) sceIoClose(logfd);
    sceKernelExitDeleteThread(0);
    for(;;) sceKernelSleepThread();
}
int main(int argc,char **argv) {
    int id,free_id=0,r,zero=0; netData data;
    logfd=sceIoOpen("host0:/street-profile.log",PSP_O_WRONLY|PSP_O_CREAT|PSP_O_TRUNC,0666);
    if(logfd<0) finish();
    say("profile helper: inspect\n");
    for(id=1;id<=10;id++) {
        r=sceUtilityCheckNetParam(id);
        say("profile %d check %08x\n",id,r);
        if(r==(int)PSP_NETPARAM_ERROR_BAD_NETCONF) {if(!free_id)free_id=id;continue;}
        if(r<0) {say("unexpected check error; stopped\n");finish();}
        memset(&data,0,sizeof data);
        r=sceUtilityGetNetParam(id,PSP_NETPARAM_SSID,&data);
        if(r<0) {say("read SSID failed %08x\n",r);finish();}
        data.asString[sizeof data.asString-1]=0;
        say("profile %d SSID %s\n",id,data.asString);
        if(!strcmp(data.asString,"Hi-Speed USB")) {say("already exists %d\n",id);finish();}
    }
    if(argc<2 || strcmp(argv[1],"create")) {say("inspect complete; free slot %d\n",free_id);finish();}
    if(!free_id) {say("no free slot\n");finish();}
    say("creating profile %d\n",free_id);
    r=sceUtilityCreateNetParam(free_id);say("create %08x\n",r);if(r<0)finish();
#define SET(p,v) do{say("setting %d\n",p);r=sceUtilitySetNetParam(p,v);say("result %08x\n",r);if(r<0)finish();}while(0)
    SET(PSP_NETPARAM_NAME,"Hi-Speed USB");
    SET(PSP_NETPARAM_SSID,"Hi-Speed USB");
    SET(PSP_NETPARAM_SECURE,&zero);
    SET(PSP_NETPARAM_IS_STATIC_IP,&zero);
    SET(PSP_NETPARAM_MANUAL_DNS,&zero);
    SET(PSP_NETPARAM_USE_PROXY,&zero);
    say("saving profile %d\n",free_id);
    r=sceUtilityCopyNetParam(0,free_id);say("save %08x\n",r);
    if(!r) {memset(&data,0,sizeof data);r=sceUtilityGetNetParam(free_id,PSP_NETPARAM_SSID,&data);say("verify %08x SSID %.127s\n",r,data.asString);}
    finish();
}
