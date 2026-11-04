// Hote des mods en script (ROADMAP-MODS.md, etape 6) : une bibliotheque comme un mod natif, qui ne connait que nds_mod.h, et fait
// tourner des scripts Luau. Le launcher la donne comme "plugin" de chaque mod en script, avec le chemin du script (reglage __script) et
// ses permissions (__permissions). Un etat Luau par mod, bibliotheques en lecture seule (luaL_sandbox), pas d'acces aux fichiers sans la
// permission "files". Pas de budget de temps : seul un rappel qui ne rend pas la main en WATCHDOG_SECONDS est arrete, et le mod retire.
// SPDX-License-Identifier: MIT
#include <array>
#include <chrono>
#include <cstdlib>
#include <cstdio>
#include <cstring>
#include <fstream>
#include <mutex>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

extern "C" {
#include "../sdk/nds_mod.h"
}
#include "lua.h"
#include "luacode.h"
#include "lualib.h"

#define HOST_VERSION "1"
static const double WATCHDOG_SECONDS = 10.0;

namespace {

struct Script;

// Une accroche de fonction : la bibliotheque donne au runtime une fonction C par accroche (THUNKS au plus par processus).
struct HookSlot {
    Script *script = nullptr;
    int ref = 0;
    nds_fn original = nullptr;
};
constexpr int THUNKS = 1024;
HookSlot slots[THUNKS];
int slots_used;

struct Script {
    const nds_api *api = nullptr;
    nds_mod *mod = nullptr;
    lua_State *L = nullptr;  // etat principal
    lua_State *T = nullptr;  // fil du script (son environnement, sur les bibliotheques en lecture seule)
    std::recursive_mutex lock;
    std::chrono::steady_clock::time_point deadline;
    bool watching = false, dead = false, files = false;
    std::vector<int> frame, gx, polygon, present, audio, io_read, io_write, irq, rom_read;
    // ce qu'un rappel peut toucher pendant qu'il tourne
    nds_u32 *top = nullptr, *bottom = nullptr;
    int w = 0, h = 0, stride = 0;
    nds_s16 *samples = nullptr;
    nds_u32 sample_count = 0;
    nds_u8 *rom = nullptr;
    nds_u32 rom_size = 0;
};

Script *of(lua_State *L) { return static_cast<Script *>(lua_getthreaddata(lua_mainthread(L))); }

void die(Script *s, const std::string &why) {
    if (s->dead) return;
    s->dead = true;
    s->api->fail(s->mod, why.c_str());
}

// Luau appelle ceci entre deux instructions : un rappel qui tourne depuis trop longtemps est arrete
void interrupt(lua_State *L, int gc) {
    if (gc >= 0) return;
    Script *s = of(L);
    if (s->watching && std::chrono::steady_clock::now() > s->deadline) {
        s->watching = false;
        lua_rawcheckstack(L, 1);
        luaL_errorL(L, "boucle infinie : le script ne rend pas la main depuis %g s", WATCHDOG_SECONDS);
    }
}

// Appelle la fonction en haut de la pile de T avec nargs arguments ; nresults resultats restent sur la pile si ca marche
bool call(Script *s, int nargs, int nresults) {
    s->deadline = std::chrono::steady_clock::now() + std::chrono::duration_cast<std::chrono::steady_clock::duration>(std::chrono::duration<double>(WATCHDOG_SECONDS));
    s->watching = true;
    int status = lua_pcall(s->T, nargs, nresults, 0);
    s->watching = false;
    if (status == LUA_OK) return true;
    std::string message = lua_isstring(s->T, -1) ? lua_tostring(s->T, -1) : "erreur sans message";
    lua_pop(s->T, 1);
    die(s, "script arrete : " + message);
    return false;
}

void push_ref(Script *s, int ref) { lua_getref(s->T, ref); }

// ------------------------------------------------------------------ la table nds
nds_u32 arg_u32(lua_State *L, int i) { return static_cast<nds_u32>(static_cast<long long>(luaL_checknumber(L, i))); }

int l_log(lua_State *L) {
    Script *s = of(L);
    std::string line;
    for (int i = 1, n = lua_gettop(L); i <= n; i++) {
        size_t len;
        const char *text = luaL_tolstring(L, i, &len);
        if (i > 1) line += ' ';
        line.append(text, len);
        lua_pop(L, 1);
    }
    s->api->log(s->mod, "%s", line.c_str());
    return 0;
}

int l_read8(lua_State *L) { lua_pushnumber(L, of(L)->api->read8(arg_u32(L, 1))); return 1; }
int l_read16(lua_State *L) { lua_pushnumber(L, of(L)->api->read16(arg_u32(L, 1))); return 1; }
int l_read32(lua_State *L) { lua_pushnumber(L, of(L)->api->read32(arg_u32(L, 1))); return 1; }
int l_write8(lua_State *L) { of(L)->api->write8(arg_u32(L, 1), arg_u32(L, 2)); return 0; }
int l_write16(lua_State *L) { of(L)->api->write16(arg_u32(L, 1), arg_u32(L, 2)); return 0; }
int l_write32(lua_State *L) { of(L)->api->write32(arg_u32(L, 1), arg_u32(L, 2)); return 0; }

int l_setting(lua_State *L) {
    const char *v = of(L)->api->setting(of(L)->mod, luaL_checkstring(L, 1));
    if (v) lua_pushstring(L, v);
    else lua_pushnil(L);
    return 1;
}

// nds.call(adresse, r0, r1, r2, r3) : appelle une fonction du jeu, rend r0, r1
int l_call(lua_State *L) {
    nds_fn f = of(L)->api->function_at(arg_u32(L, 1));
    if (!f) luaL_errorL(L, "pas de fonction du jeu a %x", arg_u32(L, 1));
    nds_u32 r[4] = {0, 0, 0, 0};
    for (int i = 0; i < 4; i++)
        if (lua_gettop(L) >= i + 2) r[i] = arg_u32(L, i + 2);
    nds_u64 out = f(r[0], r[1], r[2], r[3], 0, 0, 0, 0, 0, 0, 0, 0, 0, 0);
    lua_pushnumber(L, static_cast<nds_u32>(out));
    lua_pushnumber(L, static_cast<nds_u32>(out >> 32));
    return 2;
}

int l_frame(lua_State *L) {
    auto *frame = static_cast<nds_u32 *>(of(L)->api->runtime("nds_frame"));
    lua_pushnumber(L, frame ? *frame : 0);
    return 1;
}

int l_input(lua_State *L) {
    nds_input in;
    in.size = sizeof in;
    of(L)->api->input(&in);
    lua_createtable(L, 0, 10);
    auto field = [&](const char *name, double v) { lua_pushnumber(L, v); lua_setfield(L, -2, name); };
    field("ds_keys", in.ds_keys);
    field("touch_down", in.touch_down);
    field("touch_x", in.touch_x);
    field("touch_y", in.touch_y);
    field("mouse_x", in.mouse_dx);
    field("mouse_y", in.mouse_dy);
    field("mouse_buttons", in.mouse_buttons);
    field("wheel", in.wheel);
    field("cursor_held", in.window & 1);
    return 1;
}

int l_key_down(lua_State *L) {
    nds_input in;
    in.size = sizeof in;
    of(L)->api->input(&in);
    nds_u32 code = arg_u32(L, 1);
    lua_pushboolean(L, code < 256 && NDS_KEY_DOWN(&in, code));
    return 1;
}

int l_take_input(lua_State *L) { of(L)->api->take_input(of(L)->mod, lua_toboolean(L, 1)); return 0; }
int l_capture_mouse(lua_State *L) { of(L)->api->capture_mouse(of(L)->mod, lua_toboolean(L, 1)); return 0; }
int l_notice(lua_State *L) { of(L)->api->notice(of(L)->mod, luaL_checkstring(L, 1), static_cast<nds_u32>(luaL_optnumber(L, 2, 3000))); return 0; }

// ---- pendant on_present, on_audio, on_rom_read : les donnees du rappel en cours
int l_pixel(lua_State *L) {
    Script *s = of(L);
    if (!s->top) luaL_errorL(L, "nds.pixel : seulement dans on_present");
    nds_u32 *screen = luaL_checkinteger(L, 1) ? s->bottom : s->top;
    int x = luaL_checkinteger(L, 2), y = luaL_checkinteger(L, 3);
    if (x < 0 || y < 0 || x >= s->w || y >= s->h) return 0;
    if (lua_gettop(L) >= 4) {
        screen[y * s->stride + x] = arg_u32(L, 4);
        return 0;
    }
    lua_pushnumber(L, screen[y * s->stride + x]);
    return 1;
}

int l_sample(lua_State *L) {
    Script *s = of(L);
    if (!s->samples) luaL_errorL(L, "nds.sample : seulement dans on_audio");
    nds_u32 i = arg_u32(L, 1);
    if (i >= s->sample_count) return 0;
    if (lua_gettop(L) >= 3) {
        s->samples[2 * i] = static_cast<nds_s16>(luaL_checkinteger(L, 2));
        s->samples[2 * i + 1] = static_cast<nds_s16>(luaL_checkinteger(L, 3));
        return 0;
    }
    lua_pushinteger(L, s->samples[2 * i]);
    lua_pushinteger(L, s->samples[2 * i + 1]);
    return 2;
}

int l_rom_byte(lua_State *L) {
    Script *s = of(L);
    if (!s->rom) luaL_errorL(L, "nds.rom_byte : seulement dans on_rom_read");
    nds_u32 i = arg_u32(L, 1);
    if (i >= s->rom_size) return 0;
    if (lua_gettop(L) >= 2) {
        s->rom[i] = static_cast<nds_u8>(arg_u32(L, 2));
        return 0;
    }
    lua_pushinteger(L, s->rom[i]);
    return 1;
}

// ---- fichiers : seulement avec la permission "files", acceptee par le joueur a l'installation
int l_read_file(lua_State *L) {
    if (!of(L)->files) luaL_errorL(L, "nds.read_file : le mod n'a pas la permission \"files\"");
    std::ifstream f(luaL_checkstring(L, 1), std::ios::binary);
    if (!f) return 0;
    std::stringstream text;
    text << f.rdbuf();
    std::string all = text.str();
    lua_pushlstring(L, all.data(), all.size());
    return 1;
}

int l_write_file(lua_State *L) {
    if (!of(L)->files) luaL_errorL(L, "nds.write_file : le mod n'a pas la permission \"files\"");
    size_t len;
    const char *data = luaL_checklstring(L, 2, &len);
    std::ofstream f(luaL_checkstring(L, 1), std::ios::binary);
    f.write(data, static_cast<std::streamsize>(len));
    lua_pushboolean(L, static_cast<bool>(f));
    return 1;
}

// ------------------------------------------------------------------ rappels du moteur
int add_callback(lua_State *L, std::vector<int> Script::*list, void (*install)(Script *)) {
    Script *s = of(L);
    luaL_checktype(L, 1, LUA_TFUNCTION);
    if ((s->*list).empty()) install(s);
    (s->*list).push_back(lua_ref(L, 1));
    return 0;
}

#define EACH(list) \
    Script *s = static_cast<Script *>(user); \
    std::lock_guard<std::recursive_mutex> guard(s->lock); \
    if (s->dead) return; \
    for (int ref : s->list)

#define EACH_RET(list, none) \
    Script *s = static_cast<Script *>(user); \
    std::lock_guard<std::recursive_mutex> guard(s->lock); \
    if (s->dead) return none; \
    for (int ref : s->list)

void cb_frame(void *user, nds_u32 frame) {
    EACH(frame) {
        push_ref(s, ref);
        lua_pushnumber(s->T, frame);
        if (!call(s, 1, 0)) return;
    }
}

int cb_gx(void *user, nds_u32 cmd, nds_u32 *params, nds_u32 count) {
    EACH_RET(gx, 0) {
        push_ref(s, ref);
        lua_pushnumber(s->T, cmd);
        lua_createtable(s->T, static_cast<int>(count), 0);
        for (nds_u32 i = 0; i < count; i++) {
            lua_pushnumber(s->T, params[i]);
            lua_rawseti(s->T, -2, static_cast<int>(i + 1));
        }
        lua_pushvalue(s->T, -1);
        lua_insert(s->T, -4); // une copie de la table reste sous l'appel, pour relire les parametres changes
        if (!call(s, 2, 1)) { lua_pop(s->T, 1); return 0; }
        bool drop = lua_toboolean(s->T, -1);
        lua_pop(s->T, 1);
        for (nds_u32 i = 0; i < count; i++) {
            lua_rawgeti(s->T, -1, static_cast<int>(i + 1));
            if (lua_isnumber(s->T, -1)) params[i] = static_cast<nds_u32>(static_cast<long long>(lua_tonumber(s->T, -1)));
            lua_pop(s->T, 1);
        }
        lua_pop(s->T, 1);
        if (drop) return 1;
    }
    return 0;
}

int cb_polygon(void *user, nds_vertex *v, int n, nds_u32 attr, nds_u32 tex, nds_u32 pal) {
    static const char *names[9] = {"x", "y", "z", "w", "r", "g", "b", "s", "t"};
    EACH_RET(polygon, 0) {
        push_ref(s, ref);
        lua_createtable(s->T, n, 0);
        for (int i = 0; i < n; i++) {
            lua_createtable(s->T, 0, 9);
            const float *f = &v[i].x;
            for (int k = 0; k < 9; k++) { lua_pushnumber(s->T, f[k]); lua_setfield(s->T, -2, names[k]); }
            lua_rawseti(s->T, -2, i + 1);
        }
        lua_pushvalue(s->T, -1);
        lua_insert(s->T, -3);
        lua_pushnumber(s->T, attr);
        lua_pushnumber(s->T, tex);
        lua_pushnumber(s->T, pal);
        if (!call(s, 4, 1)) { lua_pop(s->T, 1); return 0; }
        bool drop = lua_toboolean(s->T, -1);
        lua_pop(s->T, 1);
        for (int i = 0; i < n; i++) {
            lua_rawgeti(s->T, -1, i + 1);
            float *f = &v[i].x;
            for (int k = 0; k < 9; k++) {
                lua_getfield(s->T, -1, names[k]);
                if (lua_isnumber(s->T, -1)) f[k] = static_cast<float>(lua_tonumber(s->T, -1));
                lua_pop(s->T, 1);
            }
            lua_pop(s->T, 1);
        }
        lua_pop(s->T, 1);
        if (drop) return 1;
    }
    return 0;
}

void cb_present(void *user, nds_u32 *top, nds_u32 *bottom, int w, int h, int stride) {
    EACH(present) {
        s->top = top, s->bottom = bottom, s->w = w, s->h = h, s->stride = stride;
        push_ref(s, ref);
        lua_pushinteger(s->T, w);
        lua_pushinteger(s->T, h);
        bool ok = call(s, 2, 0);
        s->top = s->bottom = nullptr;
        if (!ok) return;
    }
}

void cb_audio(void *user, nds_s16 *stereo, nds_u32 frames) {
    EACH(audio) {
        s->samples = stereo, s->sample_count = frames;
        push_ref(s, ref);
        lua_pushnumber(s->T, frames);
        bool ok = call(s, 1, 0);
        s->samples = nullptr;
        if (!ok) return;
    }
}

void cb_io_read(void *user, nds_u32 addr, nds_u32 size, nds_u32 *value) {
    EACH(io_read) {
        push_ref(s, ref);
        lua_pushnumber(s->T, addr);
        lua_pushnumber(s->T, size);
        lua_pushnumber(s->T, *value);
        if (!call(s, 3, 1)) return;
        if (lua_isnumber(s->T, -1)) *value = static_cast<nds_u32>(static_cast<long long>(lua_tonumber(s->T, -1)));
        lua_pop(s->T, 1);
    }
}

int cb_io_write(void *user, nds_u32 addr, nds_u32 size, nds_u32 *value) {
    EACH_RET(io_write, 0) {
        push_ref(s, ref);
        lua_pushnumber(s->T, addr);
        lua_pushnumber(s->T, size);
        lua_pushnumber(s->T, *value);
        if (!call(s, 3, 1)) return 0;
        bool drop = lua_isboolean(s->T, -1) && !lua_toboolean(s->T, -1); // false : l'ecriture n'a pas lieu
        if (lua_isnumber(s->T, -1)) *value = static_cast<nds_u32>(static_cast<long long>(lua_tonumber(s->T, -1)));
        lua_pop(s->T, 1);
        if (drop) return 1;
    }
    return 0;
}

void cb_irq(void *user, nds_u32 bits) {
    EACH(irq) {
        push_ref(s, ref);
        lua_pushnumber(s->T, bits);
        if (!call(s, 1, 0)) return;
    }
}

void cb_rom_read(void *user, nds_u32 pos, nds_u32 size, nds_u8 *dst) {
    EACH(rom_read) {
        s->rom = dst, s->rom_size = size;
        push_ref(s, ref);
        lua_pushnumber(s->T, pos);
        lua_pushnumber(s->T, size);
        bool ok = call(s, 2, 0);
        s->rom = nullptr;
        if (!ok) return;
    }
}

#define ON(name) \
    int l_on_##name(lua_State *L) { \
        return add_callback(L, &Script::name, [](Script *s) { s->api->on_##name(s->mod, cb_##name, s); }); \
    }
ON(frame)
ON(gx)
ON(polygon)
ON(present)
ON(audio)
ON(io_read)
ON(io_write)
ON(irq)
ON(rom_read)

// ------------------------------------------------------------------ accroches de fonctions
nds_u64 hook_dispatch(int k, nds_u32 r0, nds_u32 r1, nds_u32 r2, nds_u32 r3, nds_u32 a0, nds_u32 a1, nds_u32 a2, nds_u32 a3, nds_u32 a4,
                      nds_u32 a5, nds_u32 a6, nds_u32 a7, nds_u32 a8, nds_u32 a9) {
    HookSlot &h = slots[k];
    Script *s = h.script;
    std::unique_lock<std::recursive_mutex> guard(s->lock);
    if (s->dead) return h.original(r0, r1, r2, r3, a0, a1, a2, a3, a4, a5, a6, a7, a8, a9);
    nds_u32 stack[10] = {a0, a1, a2, a3, a4, a5, a6, a7, a8, a9};
    push_ref(s, h.ref);
    lua_pushnumber(s->T, r0);
    lua_pushnumber(s->T, r1);
    lua_pushnumber(s->T, r2);
    lua_pushnumber(s->T, r3);
    // original(r0, r1, r2, r3) : la suite (le jeu, ou le mod suivant), avec les mots de pile du jeu
    lua_pushlightuserdata(s->T, &h);
    lua_pushlightuserdata(s->T, stack);
    lua_pushcclosure(s->T, [](lua_State *L) -> int {
        auto *slot = static_cast<HookSlot *>(lua_tolightuserdata(L, lua_upvalueindex(1)));
        auto *a = static_cast<nds_u32 *>(lua_tolightuserdata(L, lua_upvalueindex(2)));
        nds_u32 r[4];
        for (int i = 0; i < 4; i++) r[i] = lua_gettop(L) > i ? arg_u32(L, i + 1) : 0;
        nds_u64 out = slot->original(r[0], r[1], r[2], r[3], a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7], a[8], a[9]);
        lua_pushnumber(L, static_cast<nds_u32>(out));
        lua_pushnumber(L, static_cast<nds_u32>(out >> 32));
        return 2;
    }, "original", 2);
    if (!call(s, 5, 2)) {
        guard.unlock();
        return h.original(r0, r1, r2, r3, a0, a1, a2, a3, a4, a5, a6, a7, a8, a9);
    }
    nds_u32 lo = lua_isnumber(s->T, -2) ? static_cast<nds_u32>(static_cast<long long>(lua_tonumber(s->T, -2))) : 0;
    nds_u32 hi = lua_isnumber(s->T, -1) ? static_cast<nds_u32>(static_cast<long long>(lua_tonumber(s->T, -1))) : 0;
    lua_pop(s->T, 2);
    return (static_cast<nds_u64>(hi) << 32) | lo;
}

template <int K>
nds_u64 thunk(nds_u32 r0, nds_u32 r1, nds_u32 r2, nds_u32 r3, nds_u32 a0, nds_u32 a1, nds_u32 a2, nds_u32 a3, nds_u32 a4, nds_u32 a5,
              nds_u32 a6, nds_u32 a7, nds_u32 a8, nds_u32 a9) {
    return hook_dispatch(K, r0, r1, r2, r3, a0, a1, a2, a3, a4, a5, a6, a7, a8, a9);
}

template <int... K>
constexpr std::array<nds_fn, sizeof...(K)> make_thunks(std::integer_sequence<int, K...>) { return {thunk<K>...}; }
const auto THUNK_TABLE = make_thunks(std::make_integer_sequence<int, THUNKS>{});

// nds.hook(adresse, fonction(r0, r1, r2, r3, original) -> r0, r1)
int l_hook(lua_State *L) {
    Script *s = of(L);
    nds_u32 addr = arg_u32(L, 1);
    luaL_checktype(L, 2, LUA_TFUNCTION);
    if (slots_used >= THUNKS) luaL_errorL(L, "nds.hook : plus de %d accroches dans ce jeu", THUNKS);
    int k = slots_used;
    slots[k].script = s;
    slots[k].ref = lua_ref(L, 2);
    if (s->api->hook(s->mod, addr, reinterpret_cast<void *>(THUNK_TABLE[k]), reinterpret_cast<void **>(&slots[k].original))) {
        lua_unref(L, slots[k].ref);
        lua_pushboolean(L, false);
        return 1;
    }
    slots_used++;
    lua_pushboolean(L, true);
    return 1;
}

// nds.hook_insn(adresse, fonction(registres)) : registres = {r0 .. r15, cpsr}, modifiables
void insn_callback(void *user, nds_regs *regs) {
    auto *pair = static_cast<std::pair<Script *, int> *>(user);
    Script *s = pair->first;
    std::lock_guard<std::recursive_mutex> guard(s->lock);
    if (s->dead) return;
    push_ref(s, pair->second);
    lua_createtable(s->T, 0, 17);
    for (int i = 0; i < 16; i++) {
        lua_pushnumber(s->T, regs->r[i]);
        lua_setfield(s->T, -2, ("r" + std::to_string(i)).c_str());
    }
    lua_pushnumber(s->T, regs->cpsr);
    lua_setfield(s->T, -2, "cpsr");
    lua_pushvalue(s->T, -1);
    lua_insert(s->T, -3);
    if (!call(s, 1, 0)) { lua_pop(s->T, 1); return; }
    for (int i = 0; i < 16; i++) {
        lua_getfield(s->T, -1, ("r" + std::to_string(i)).c_str());
        if (lua_isnumber(s->T, -1)) regs->r[i] = static_cast<nds_u32>(static_cast<long long>(lua_tonumber(s->T, -1)));
        lua_pop(s->T, 1);
    }
    lua_getfield(s->T, -1, "cpsr");
    if (lua_isnumber(s->T, -1)) regs->cpsr = static_cast<nds_u32>(static_cast<long long>(lua_tonumber(s->T, -1)));
    lua_pop(s->T, 2);
}

int l_hook_insn(lua_State *L) {
    Script *s = of(L);
    luaL_checktype(L, 2, LUA_TFUNCTION);
    auto *pair = new std::pair<Script *, int>(s, lua_ref(L, 2));
    lua_pushboolean(L, s->api->hook_insn(s->mod, arg_u32(L, 1), insn_callback, pair) == 0);
    return 1;
}

const luaL_Reg NDS[] = {
    {"log", l_log}, {"read8", l_read8}, {"read16", l_read16}, {"read32", l_read32}, {"write8", l_write8}, {"write16", l_write16},
    {"write32", l_write32}, {"setting", l_setting}, {"call", l_call}, {"frame", l_frame}, {"input", l_input}, {"key_down", l_key_down},
    {"take_input", l_take_input}, {"capture_mouse", l_capture_mouse}, {"notice", l_notice}, {"pixel", l_pixel}, {"sample", l_sample},
    {"rom_byte", l_rom_byte}, {"read_file", l_read_file}, {"write_file", l_write_file}, {"hook", l_hook}, {"hook_insn", l_hook_insn},
    {"on_frame", l_on_frame}, {"on_gx", l_on_gx}, {"on_polygon", l_on_polygon}, {"on_present", l_on_present}, {"on_audio", l_on_audio},
    {"on_io_read", l_on_io_read}, {"on_io_write", l_on_io_write}, {"on_irq", l_on_irq}, {"on_rom_read", l_on_rom_read},
    {nullptr, nullptr},
};

bool read_text(const std::string &path, std::string &out) {
    std::ifstream f(path, std::ios::binary);
    if (!f) return false;
    std::stringstream text;
    text << f.rdbuf();
    out = text.str();
    return true;
}

} // namespace

extern "C" NDS_MOD_EXPORT int nds_mod_load(const nds_api *api, nds_mod *mod) {
    const char *path = api->setting(mod, "__script");
    if (!path) {
        api->fail(mod, "hote Luau : pas de script (__script)");
        return 1;
    }
    std::string permissions = api->setting(mod, "__permissions") ? api->setting(mod, "__permissions") : "";
    for (const char *p : {"network", "system"})
        if (permissions.find(p) != std::string::npos) {
            api->fail(mod, (std::string("hote Luau ") + HOST_VERSION + " : la permission \"" + p + "\" n'est pas encore prise en charge").c_str());
            return 1;
        }
    std::string source;
    if (!read_text(path, source)) {
        api->fail(mod, (std::string("script introuvable : ") + path).c_str());
        return 1;
    }
    auto *s = new Script;
    s->api = api;
    s->mod = mod;
    s->files = permissions.find("files") != std::string::npos;
    std::lock_guard<std::recursive_mutex> guard(s->lock);
    s->L = luaL_newstate();
    lua_setthreaddata(s->L, s);
    lua_callbacks(s->L)->interrupt = interrupt;
    luaL_openlibs(s->L);
    luaL_register(s->L, "nds", NDS);
    lua_pushstring(s->L, api->game_code);
    lua_setfield(s->L, -2, "game_code");
    lua_pushstring(s->L, api->platform);
    lua_setfield(s->L, -2, "platform");
    lua_pushstring(s->L, api->language ? api->language : "en");
    lua_setfield(s->L, -2, "language");
    lua_pushstring(s->L, HOST_VERSION);
    lua_setfield(s->L, -2, "host_version");
    lua_pop(s->L, 1);
    luaL_sandbox(s->L); // bibliotheques et nds en lecture seule
    s->T = lua_newthread(s->L);
    lua_ref(s->L, -1);
    luaL_sandboxthread(s->T);
    size_t size = 0;
    char *bytecode = luau_compile(source.data(), source.size(), nullptr, &size);
    int status = luau_load(s->T, path, bytecode, size, 0);
    std::free(bytecode);
    if (status != 0) {
        std::string message = lua_tostring(s->T, -1);
        api->fail(mod, ("script illisible : " + message).c_str());
        return 1;
    }
    if (!call(s, 0, 0)) return 1;
    return s->dead ? 1 : 0;
}

