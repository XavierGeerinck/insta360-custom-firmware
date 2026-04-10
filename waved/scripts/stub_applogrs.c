// Minimal stub for libapplogrs.so — all telemetry functions are no-ops.
// This allows pspapp to start (for display/touch init) without sending telemetry.

void AppLog_init_rangers(void) {}
void AppLog_onEvent(void) {}
void AppLog_registerBdDidObserver(void) {}
void AppLog_registerSsidObserver(void) {}
void AppLog_setCustomDomain(void) {}
void AppLog_setDbDir(void) {}
void AppLog_setHeader(void) {}
void AppLog_setLogEnabled(void) {}
void AppLog_setUserUniqueId(void) {}
