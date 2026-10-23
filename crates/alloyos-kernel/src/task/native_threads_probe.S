// Real protected thread primitives; this is deliberately not a pthread shim.
.section .text.native_threads_probe,"ax"
.global native_threads_probe_start
.global native_threads_probe_end
native_threads_probe_start:
    mov x21, #0x1000000000
    add x20, x21, #0x1000
    add x0, x21, #0x3000
    msr tpidr_el0, x0
    mov x1, #11
    str x1, [x0]
    // Fill the bounded create descriptor in shared writable memory.
    adr x0, native_threads_child
    str x0, [x20, #256]
    str x20, [x20, #264]
    add x0, x21, #0x4000
    str x0, [x20, #272]
    mov x0, #0x4000
    str x0, [x20, #280]
    add x0, x21, #0x2000
    str x0, [x20, #288]
    str xzr, [x20, #296]
    add x0, x20, #256
    mov x1, #48
    mov x2, #0
    mov x8, #408
    svc #0
    cmp x0, #0
    b.le native_threads_fail
    mov x22, x0
    // Wait can race the child's release-store; EAGAIN means no lost wake.
    mov x0, x20
    mov x1, #0
    mov x2, #0
    mov x8, #411
    svc #0
    cbz x0, 1f
    cmn x0, #11
    b.ne native_threads_fail
1:  ldar w0, [x20]
    cmp w0, #1
    b.ne native_threads_fail
    ldr w0, [x20, #12]
    cmp w0, #55
    b.ne native_threads_fail
    mrs x0, tpidr_el0
    ldr x0, [x0]
    cmp x0, #11
    b.ne native_threads_fail
    add x0, x20, #4
    mov w1, #1
    stlr w1, [x0]
    mov x1, #1
    mov x2, #0
    mov x8, #412
    svc #0
    cmp x0, #0
    b.lt native_threads_fail
    mov x23, x0
    add x0, x20, #8
    mov x1, #0
    mov x2, #0
    mov x8, #411
    svc #0
    cbz x0, 2f
    cmn x0, #11
    b.ne native_threads_fail
2:  add x0, x20, #8
    ldar w0, [x0]
    cmp w0, #1
    b.ne native_threads_fail
    // A stale expected value must return EAGAIN rather than sleep.
    mov x0, x20
    mov x1, #0
    mov x2, #0
    mov x8, #411
    svc #0
    cmn x0, #11
    b.ne native_threads_fail
    // Deadline while a word remains zero; clock_gettime validates the same space.
    mov x0, #1
    add x1, x20, #64
    mov x8, #228
    svc #0
    cbnz x0, native_threads_fail
    ldr x0, [x20, #64]
    ldr x1, =1000000000
    mul x0, x0, x1
    ldr x1, [x20, #72]
    add x2, x0, x1
    ldr x1, =5000000
    add x2, x2, x1
    add x0, x20, #16
    mov x1, #0
    mov x8, #411
    svc #0
    cmn x0, #110
    b.ne native_threads_fail
    // Join is nonblocking until actual child retirement, then returns its status.
3:  mov x0, x22
    add x1, x20, #40
    mov x2, #0
    mov x8, #410
    svc #0
    cbz x0, 4f
    cmn x0, #11
    b.ne native_threads_fail
    mov x8, #24
    svc #0
    b 3b
4:  // Join guarantees the child's final wake counter has been published.
    ldr x0, [x20, #48]
    ldr x1, [x20, #56]
    add x0, x0, x1
    add x0, x0, x23
    cbz x0, native_threads_fail
    ldr w0, [x20, #40]
    cmp w0, #17
    b.ne native_threads_fail
    mov x0, #0
    mov x8, #60
    svc #0
    b .
native_threads_fail:
    mov x0, #93
    mov x8, #60
    svc #0
    b .
native_threads_child:
    mov x20, x0
    mrs x21, tpidr_el0
    mov x0, #22
    str x0, [x21]
    mov w0, #55
    str w0, [x20, #12]
    mov w0, #1
    stlr w0, [x20]
    mov x0, x20
    mov x1, #1
    mov x2, #0
    mov x8, #412
    svc #0
    cmp x0, #0
    b.lt native_threads_child_fail
    str x0, [x20, #48]
    add x0, x20, #4
    mov x1, #0
    mov x2, #0
    mov x8, #411
    svc #0
    cbz x0, 5f
    cmn x0, #11
    b.ne native_threads_child_fail
5:  mrs x0, tpidr_el0
    cmp x0, x21
    b.ne native_threads_child_fail
    ldr x0, [x0]
    cmp x0, #22
    b.ne native_threads_child_fail
    add x0, x20, #8
    mov w1, #1
    stlr w1, [x0]
    mov x1, #1
    mov x2, #0
    mov x8, #412
    svc #0
    cmp x0, #0
    b.lt native_threads_child_fail
    str x0, [x20, #56]
    mov x0, #17
    mov x1, #0
    mov x2, #0
    mov x8, #409
    svc #0
    b .
native_threads_child_fail:
    mov x0, #94
    mov x8, #60
    svc #0
    b .
.ltorg
native_threads_probe_end:

.global native_threads_fault_start
.global native_threads_fault_end
native_threads_fault_start:
    mov x21, #0x1000000000
    add x20, x21, #0x1000
    adr x0, native_threads_fault_child
    str x0, [x20, #256]
    str x20, [x20, #264]
    add x0, x21, #0x4000
    str x0, [x20, #272]
    mov x0, #0x4000
    str x0, [x20, #280]
    add x0, x21, #0x2000
    str x0, [x20, #288]
    str xzr, [x20, #296]
    add x0, x20, #256
    mov x1, #48
    mov x2, #0
    mov x8, #408
    svc #0
    cmp x0, #0
    b.le 6f
    mov x0, x20
    mov x1, #0
    mov x2, #0
    mov x8, #411
    svc #0
6:  mov x0, #95
    mov x8, #60
    svc #0
    b .
native_threads_fault_child:
    mov x0, #0x40000000
    ldr x0, [x0]
    mov x0, #96
    mov x8, #60
    svc #0
    b .
native_threads_fault_end:
