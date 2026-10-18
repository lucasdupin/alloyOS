// Position-independent EL0 acceptance program. x19 is the owned reservation.
.section .text.web_runtime_probe,"ax"
.global web_runtime_probe_start
.global web_runtime_probe_end
web_runtime_probe_start:
    mov x0, #1
    mov x1, #0
    mov x2, #0
    mov x8, #404
    svc #0
    cmn x0, #22
    b.ne 9f
    mov x0, #8192
    mov x1, #0
    mov x2, #0
    mov x8, #404
    svc #0
    cmp x0, #0
    b.le 9f
    mov x19, x0
    mov x1, #8192
    mov x2, #3
    mov x8, #405
    svc #0
    cbnz x0, 9f
    ldr x0, [x19]
    cbnz x0, 9f
    mov x0, #42
    str x0, [x19]
    mov x0, x19
    mov x1, #8192
    mov x2, #3
    mov x8, #405
    svc #0
    cbnz x0, 9f
    ldr x0, [x19]
    cmp x0, #42
    b.ne 9f
    mov x0, x19
    mov x1, #8192
    mov x2, #0
    mov x8, #406
    svc #0
    cbnz x0, 9f
    mov x0, x19
    mov x1, #8192
    mov x2, #3
    mov x8, #405
    svc #0
    cbnz x0, 9f
    ldr x0, [x19]
    cbnz x0, 9f
    mov x0, #77
    str x0, [x19]
    // Executable permission is outside this private-memory ABI.
    mov x0, x19
    mov x1, #8192
    mov x2, #7
    mov x8, #405
    svc #0
    cmn x0, #22
    b.ne 9f
    mov x0, x19
    mov x1, #8192
    mov x2, #0
    mov x8, #407
    svc #0
    cbnz x0, 9f
    mov x0, x19
    mov x1, #8192
    mov x2, #3
    mov x8, #405
    svc #0
    cmn x0, #22
    b.ne 9f
    // Existing executable pages can never become writable through this ABI.
    mov x0, #0x1000000000
    mov x1, #4096
    mov x2, #3
    mov x8, #405
    svc #0
    cmn x0, #22
    b.ne 9f
    // Reservation exceeding the process page quota fails without allocation.
    mov x0, #0x1000000
    mov x1, #0
    mov x2, #0
    mov x8, #404
    svc #0
    cmn x0, #12
    b.ne 9f
    mov x0, #0
    b 10f
9:  mov x0, #91
10: mov x8, #60
    svc #0
    b .
web_runtime_probe_end:

.global web_runtime_guard_start
.global web_runtime_guard_end
web_runtime_guard_start:
    mov x0, #4096
    mov x1, #0
    mov x2, #0
    mov x8, #404
    svc #0
    cmp x0, #0
    b.le 11f
    ldr x1, [x0]
11: mov x0, #92
    mov x8, #60
    svc #0
    b .
web_runtime_guard_end:
