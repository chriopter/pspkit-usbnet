	.set noreorder
#include "pspimport.s"
	IMPORT_START "sceNetApctl_internal_user",0x00090000
	IMPORT_FUNC  "sceNetApctl_internal_user",0xE9B2E5E6,sceNetApctlScanUser
	IMPORT_FUNC  "sceNetApctl_internal_user",0x6BDDCB8C,sceNetApctlGetBSSDescIDListUser
	IMPORT_FUNC  "sceNetApctl_internal_user",0x04776994,sceNetApctlGetBSSDescEntryUser
