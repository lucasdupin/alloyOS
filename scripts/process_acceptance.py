"""Transcript gates shared by the protected-process QEMU checks.

These validate guest evidence; fixture tests of this module do not run a guest.
"""
RUNTIME_MARKERS = (b'APP-PROCESS: hostile package parser limits and host recovery PASS',
           b'APP-PROCESS: persistent EL0 handler and filesystem IPC PASS',
           b'APP-PROCESS: denied peer filesystem capability PASS',
           b'APP-PROCESS: bounded infinite loop and independent peer PASS',
           b'APP-PROCESS: process restart PASS',
           b'APP-PROCESS: Tasks scheduler IPC persistence PASS',
           b'APP-PROCESS: bounded host-call flood and budget reset PASS',
           b'APP-PROCESS: private heap exhaustion and restart PASS',
           b'APP-PROCESS: display input and shared kernel operation denial PASS',
           b'APP-PROCESS: timer cancels nonyielding process and peer continues PASS',
           b'DISPLAY-PROCESS: real EL0 fault, restart, conversation/dashboard restoration PASS',
           b'WELDER-SERVICE: protected setup transition, fault/timeout restart and replay rejection PASS')
ISOLATION_MARKER = b'[process-selftest] PASS: EL0 syscalls, IPC, invalid pointers, privileged memory fault, timer preemption and kernel survival'
FATAL_MARKERS = (b'[crash] kernel-panic ', b'[crash] exception domain=kernel ')


def has_fatal_failure(output):
    return any(line.startswith(FATAL_MARKERS) for line in output.splitlines())


def acceptance_errors(output, *, isolation=False):
    # Match complete serial lines, allowing QEMU/PL011 CRLF endings. A truncated
    # or embedded marker must never turn an incomplete probe into acceptance.
    lines = [line.rstrip(b'\r') for line in output.split(b'\n')[:-1]]
    required = ((b'intentional EL0 panic', ISOLATION_MARKER)
                if isolation else RUNTIME_MARKERS)
    errors = []
    if has_fatal_failure(output):
        errors.append('kernel panic in guest output')
    for marker in required:
        # Panic text is emitted by the contained user process.
        present = (marker in output if marker == b'intentional EL0 panic'
                   else marker in lines)
        if not present:
            errors.append('missing: ' + marker.decode())
    if isolation:
        # The crash reporter emits the header and unwound frames separately.
        if not (any(line.startswith(b'[crash] exception domain=user ') for line in lines)
                and any(line.startswith(b'[crash] frame=2 ') for line in lines)):
            errors.append('protected-process fault diagnostics missing')
    return errors
