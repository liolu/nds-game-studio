/* Fonctions du SDK qui parlent au processeur ARM7 : ici elles repondent directement. */
#include "nitro.h"

typedef struct { u16 x, y, touch, validity; } TPData;

static TPData *auto_buf;
static u32 auto_size;

static void current_touch(TPData *d) {
    int down, x, y;
    present_touch(&down, &x, &y);
    d->x = (u16)x;
    d->y = (u16)y;
    d->touch = (u16)(down != 0);
    d->validity = 0;
}

void hle_frame(void) {
    if (auto_buf)
        for (u32 i = 0; i < auto_size; i++) current_touch(&auto_buf[i]);
}

void TP_Init(void) {}
u32 TP_GetUserInfo(void *calibrate) {
    nds_zero(calibrate, 12);
    return 1;
}
void TP_WaitBusy(u32 mask) {}
u32 TP_RequestSetStabilityAsync(u32 a, u32 b) { return 0; }
u32 TP_RequestSamplingAsync(void) { return 0; }
u32 TP_WaitCalibratedResult(TPData *d) {
    current_touch(d);
    return 0;
}
void TP_GetCalibratedPoint(TPData *disp, const TPData *raw) { *disp = *raw; }
u32 TP_RequestAutoSamplingStartAsync(u32 vcount, u32 freq, TPData *buf, u32 size) {
    auto_buf = buf;
    auto_size = size & 0xFFFF;
    return 0;
}
u32 TP_RequestAutoSamplingStopAsync(void) {
    auto_buf = 0;
    return 0;
}

/* alimentation */
static u32 backlight = 3;
u32 PM_SetBackLightAsync(u32 a, u32 b, void *cb, void *arg) {
    if (cb) ((void (*)(u32, void *))cb)(0, arg);
    return 0;
}
u32 PM_SetBackLight(u32 a, u32 b) { return 0; }
u32 PM_GetBackLight(u32 *top, u32 *bottom) {
    if (top) *top = backlight;
    if (bottom) *bottom = backlight;
    return 0;
}
u32 PM_SetLCDPower(u32 on) { return 1; }
u32 PM_GetLCDPower(void) { return 1; }
u32 PM_ForceToPowerOffAsync(void *cb, void *arg) { return 0; }
u32 PM_ForceToPowerOff(void) { ExitProcess(0); return 0; }

/* horloge */
typedef struct { u16 year, month, dow, day, hour, minute, second, ms; } SYSTEMTIME;
IMPORT void WINAPI GetLocalTime(SYSTEMTIME *);
typedef struct { u32 year, month, day, week; } RTCDate;
typedef struct { u32 hour, minute, second; } RTCTime;

void RTC_Init(void) {}
u32 RTC_GetDateTime(RTCDate *d, RTCTime *t) {
    SYSTEMTIME s;
    GetLocalTime(&s);
    if (d) { d->year = s.year % 100; d->month = s.month; d->day = s.day; d->week = s.dow; }
    if (t) { t->hour = s.hour; t->minute = s.minute; t->second = s.second; }
    return 0;
}
u32 RTC_GetDateTimeAsync(RTCDate *d, RTCTime *t, void (*cb)(u32, void *), void *arg) {
    RTC_GetDateTime(d, t);
    if (cb) cb(0, arg);
    return 0;
}

/* lecture de fichiers : la rom est deja en memoire, resultat immediat (FS_RESULT_SUCCESS = 0) */
u32 FSi_ReadRomCallback(void *arc, void *dst, u32 pos, u32 size) {
    if (pos + size > nds_rom_size) nds_fatal("lecture hors de la rom : %x + %x", pos, size);
    nds_copy(dst, nds_rom + pos, size);
    return 0;
}
