# CMake cross-compilation toolchain file for native alloyOS AArch64 protected userland.
#
# Usage:
#   cmake -S /tmp/wpewebkit-2.54.1 -B /tmp/alloyos-jsc -G Ninja \
#     -DCMAKE_TOOLCHAIN_FILE=/absolute/path/to/scripts/web-engine/alloyos-toolchain.cmake \
#     -DALLOYOS_SYSROOT=/private/tmp/alloyos-sysroot \
#     -DPORT=JSCOnly -DCMAKE_BUILD_TYPE=Release \
#     -DENABLE_STATIC_JSC=ON -DEVENT_LOOP_TYPE=Generic \
#     -DENABLE_JSC_GLIB_API=OFF -DENABLE_REMOTE_INSPECTOR=OFF \
#     -DENABLE_JIT=OFF -DENABLE_DFG_JIT=OFF -DENABLE_FTL_JIT=OFF \
#     -DENABLE_C_LOOP=ON -DENABLE_WEBASSEMBLY=OFF \
#     -DENABLE_SAMPLING_PROFILER=OFF -DENABLE_API_TESTS=OFF

list(APPEND CMAKE_MODULE_PATH "${CMAKE_CURRENT_LIST_DIR}/cmake")
set(CMAKE_SYSTEM_NAME AlloyOS)
# pthreads are part of liballoyos_posix.a; stop FindThreads from adding -lpthreads.
set(CMAKE_THREAD_LIBS_INIT "" CACHE STRING "")
set(CMAKE_HAVE_THREADS_LIBRARY 1 CACHE INTERNAL "")
set(CMAKE_USE_PTHREADS_INIT 1 CACHE INTERNAL "")
set(Threads_FOUND TRUE CACHE INTERNAL "")
set(CMAKE_SYSTEM_PROCESSOR aarch64)
set(CMAKE_CROSSCOMPILING TRUE)

# Compiler sanity checks compile static objects rather than hosted Darwin/Linux executables.
set(CMAKE_TRY_COMPILE_TARGET_TYPE STATIC_LIBRARY)

if(NOT DEFINED ALLOYOS_SYSROOT OR "${ALLOYOS_SYSROOT}" STREQUAL "")
    if(DEFINED ENV{ALLOYOS_SYSROOT} AND NOT "$ENV{ALLOYOS_SYSROOT}" STREQUAL "")
        set(ALLOYOS_SYSROOT "$ENV{ALLOYOS_SYSROOT}")
    else()
        set(ALLOYOS_SYSROOT "/private/tmp/alloyos-sysroot")
    endif()
endif()

get_filename_component(ALLOYOS_SYSROOT "${ALLOYOS_SYSROOT}" ABSOLUTE)

foreach(_required_file
    "${ALLOYOS_SYSROOT}/include/alloyos/syscall.h"
    "${ALLOYOS_SYSROOT}/include/pthread.h"
    "${ALLOYOS_SYSROOT}/include/sys/mman.h"
    "${ALLOYOS_SYSROOT}/lib/liballoyos_posix.a"
    "${ALLOYOS_SYSROOT}/lib/liballoyos_cxxrt.a"
    "${ALLOYOS_SYSROOT}/lib/process.ld")
    if(NOT EXISTS "${_required_file}")
        message(FATAL_ERROR
            "alloyOS sysroot is incomplete: missing ${_required_file}. "
            "Build it first with: python3 scripts/web-engine/build-sysroot.py --output ${ALLOYOS_SYSROOT}")
    endif()
endforeach()

set(CMAKE_SYSROOT "${ALLOYOS_SYSROOT}")
set(CMAKE_RANLIB "${CMAKE_CURRENT_LIST_DIR}/bin/ranlib" CACHE FILEPATH "alloyOS ranlib shim")
set(ALLOYOS_TARGET_TRIPLE "aarch64-none-elf")

set(CMAKE_C_COMPILER_TARGET "${ALLOYOS_TARGET_TRIPLE}")
set(CMAKE_CXX_COMPILER_TARGET "${ALLOYOS_TARGET_TRIPLE}")
set(CMAKE_ASM_COMPILER_TARGET "${ALLOYOS_TARGET_TRIPLE}")

if(NOT CMAKE_C_COMPILER)
    find_program(CMAKE_C_COMPILER NAMES clang REQUIRED)
endif()
if(NOT CMAKE_CXX_COMPILER)
    find_program(CMAKE_CXX_COMPILER NAMES clang++ REQUIRED)
endif()

if(NOT CMAKE_AR OR NOTALLOYOS_RUST_LLD)
    find_program(_ALLOYOS_RUSTC NAMES rustc)
    if(_ALLOYOS_RUSTC)
        execute_process(
            COMMAND "${_ALLOYOS_RUSTC}" --print sysroot
            OUTPUT_VARIABLE _RUST_SYSROOT
            OUTPUT_STRIP_TRAILING_WHITESPACE)
        execute_process(
            COMMAND "${_ALLOYOS_RUSTC}" -vV
            OUTPUT_VARIABLE _RUST_VV)
        string(REGEX MATCH "host: ([^\n\r]+)" _ "${_RUST_VV}")
        set(_RUST_HOST "${CMAKE_MATCH_1}")
        set(_RUST_BIN "${_RUST_SYSROOT}/lib/rustlib/${_RUST_HOST}/bin")
        if(NOT CMAKE_AR AND EXISTS "${_RUST_BIN}/llvm-ar")
            set(CMAKE_AR "${_RUST_BIN}/llvm-ar" CACHE FILEPATH "alloyOS archiver")
        endif()
        if(NOT ALLOYOS_RUST_LLD AND EXISTS "${_RUST_BIN}/rust-lld")
            set(ALLOYOS_RUST_LLD "${_RUST_BIN}/rust-lld" CACHE FILEPATH "alloyOS ELF linker")
        endif()
    endif()
endif()

set(_ALLOYOS_COMMON_FLAGS
    "--target=${ALLOYOS_TARGET_TRIPLE} --sysroot=${ALLOYOS_SYSROOT} -ffreestanding -fno-stack-protector -fno-unwind-tables -fno-asynchronous-unwind-tables -fno-pic -fno-builtin -ftls-model=local-exec -D__alloyos__=1 -D_POSIX_C_SOURCE=200809L -D_XOPEN_SOURCE=700 -D_POSIX_TIMERS=1 -D__unix__=1 -D_GNU_SOURCE=1 -D_POSIX_CPUTIME=1 -D_POSIX_THREAD_CPUTIME=1 -D_POSIX_MONOTONIC_CLOCK=200809L")

set(CMAKE_C_FLAGS_INIT
    "${_ALLOYOS_COMMON_FLAGS} -isystem ${ALLOYOS_SYSROOT}/include")
set(CMAKE_CXX_FLAGS_INIT
    "${_ALLOYOS_COMMON_FLAGS} -fno-exceptions -fno-rtti -nostdinc++ -isystem ${ALLOYOS_SYSROOT}/include/c++/v1 -isystem ${ALLOYOS_SYSROOT}/include")

set(_ALLOYOS_LINK_LIBS "-L${ALLOYOS_SYSROOT}/lib -lalloyos_cxxrt -lalloyos_posix")
foreach(_lib c++ c++abi icui18n icuuc icudata)
    if(EXISTS "${ALLOYOS_SYSROOT}/lib/lib${_lib}.a")
        set(_ALLOYOS_LINK_LIBS "${_ALLOYOS_LINK_LIBS} -l${_lib}")
    endif()
endforeach()
if(EXISTS "${ALLOYOS_SYSROOT}/lib/libc.a")
    set(_ALLOYOS_LINK_LIBS "${_ALLOYOS_LINK_LIBS} -lc")
endif()
if(EXISTS "${ALLOYOS_SYSROOT}/lib/libm.a")
    set(_ALLOYOS_LINK_LIBS "${_ALLOYOS_LINK_LIBS} -lm")
endif()
# compiler-rt builtins (soft-float long double, 128-bit math) from rustup's compiler_builtins.
if(_RUST_SYSROOT)
    file(GLOB _ALLOYOS_BUILTINS "${_RUST_SYSROOT}/lib/rustlib/aarch64-unknown-none/lib/libcompiler_builtins-*.rlib")
    list(GET _ALLOYOS_BUILTINS 0 _ALLOYOS_BUILTINS_LIB)
    if(_ALLOYOS_BUILTINS_LIB)
        set(_ALLOYOS_LINK_LIBS "${_ALLOYOS_LINK_LIBS} ${_ALLOYOS_BUILTINS_LIB}")
    endif()
endif()

set(CMAKE_EXE_LINKER_FLAGS_INIT
    "-fuse-ld=lld --ld-path=${CMAKE_CURRENT_LIST_DIR}/bin/ld.lld -nostdlib -Wl,--no-undefined -Wl,--fatal-warnings -Wl,--build-id=none -Wl,-z,max-page-size=4096 -Wl,-T,${ALLOYOS_SYSROOT}/lib/process.ld -Wl,--start-group ${_ALLOYOS_LINK_LIBS} -Wl,--end-group")

# Strictly isolate package/header/library discovery to the alloyOS sysroot.
# Never treat host Darwin/Linux headers or libraries (e.g. host ICU/GLib/pthread) as target ones.
set(CMAKE_FIND_ROOT_PATH "${ALLOYOS_SYSROOT}")
set(CMAKE_FIND_ROOT_PATH_MODE_PROGRAM NEVER)
set(CMAKE_FIND_ROOT_PATH_MODE_LIBRARY ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_INCLUDE ONLY)
set(CMAKE_FIND_ROOT_PATH_MODE_PACKAGE ONLY)
set(ENV{PKG_CONFIG_DIR} "")
set(ENV{PKG_CONFIG_LIBDIR} "${ALLOYOS_SYSROOT}/lib/pkgconfig:${ALLOYOS_SYSROOT}/share/pkgconfig")
set(ENV{PKG_CONFIG_SYSROOT_DIR} "${ALLOYOS_SYSROOT}")

# Hint ICU directly to the sysroot paths for CMake's FindICU module.
if(EXISTS "${ALLOYOS_SYSROOT}/include/unicode/utypes.h")
    set(ICU_ROOT "/" CACHE PATH "ICU root within sysroot")
    set(ICU_INCLUDE_DIR "${ALLOYOS_SYSROOT}/include" CACHE PATH "ICU include directory")
    if(EXISTS "${ALLOYOS_SYSROOT}/lib/libicuuc.a")
        set(ICU_UC_LIBRARY "${ALLOYOS_SYSROOT}/lib/libicuuc.a" CACHE FILEPATH "ICU uc library")
        set(ICU_I18N_LIBRARY "${ALLOYOS_SYSROOT}/lib/libicui18n.a" CACHE FILEPATH "ICU i18n library")
        set(ICU_DATA_LIBRARY "${ALLOYOS_SYSROOT}/lib/libicudata.a" CACHE FILEPATH "ICU data library")
    endif()
endif()
