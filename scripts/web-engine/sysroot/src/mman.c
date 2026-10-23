#include <errno.h>
#include <stddef.h>
#include <stdint.h>
#include <sys/mman.h>
#include <alloyos/syscall.h>

static int align_page_length(size_t len, size_t* out_len)
{
    if (len == 0U || len > SIZE_MAX - (ALLOYOS_PAGE_SIZE - 1U)) {
        return -1;
    }
    *out_len = (len + ALLOYOS_PAGE_SIZE - 1U) & ~(ALLOYOS_PAGE_SIZE - 1U);
    return 0;
}

void* mmap(void* addr, size_t len, int prot, int flags, int fd, off_t offset)
{
    size_t aligned_len = 0;
    if (offset != 0 || align_page_length(len, &aligned_len) != 0) {
        errno = EINVAL;
        return MAP_FAILED;
    }

    int known_flags = MAP_PRIVATE | MAP_SHARED | MAP_FIXED | MAP_ANONYMOUS | MAP_NORESERVE;
    if ((flags & ~known_flags) != 0) {
        errno = EINVAL;
        return MAP_FAILED;
    }

    /* Protected processes only support anonymous private memory reservations */
    if ((flags & MAP_SHARED) != 0 || (flags & MAP_ANONYMOUS) == 0 || fd != -1) {
        errno = ENOTSUP;
        return MAP_FAILED;
    }
    if ((flags & MAP_PRIVATE) == 0) {
        errno = EINVAL;
        return MAP_FAILED;
    }

    if ((prot & ~(PROT_READ | PROT_WRITE | PROT_EXEC)) != 0) {
        errno = EINVAL;
        return MAP_FAILED;
    }
    if ((prot & PROT_EXEC) != 0) {
        errno = ENOTSUP;
        return MAP_FAILED;
    }

    /* Support MAP_FIXED only when re-committing or decommitting an existing reservation range */
    if ((flags & MAP_FIXED) != 0) {
        if (!addr || ((uintptr_t)addr & (ALLOYOS_PAGE_SIZE - 1U)) != 0U) {
            errno = EINVAL;
            return MAP_FAILED;
        }
        int64_t rc;
        if (prot == PROT_NONE) {
            rc = alloyos_syscall2(ALLOYOS_SYS_MEMORY_DECOMMIT,
                                  (uint64_t)(uintptr_t)addr,
                                  (uint64_t)aligned_len);
        } else {
            uint64_t mode = (prot & PROT_WRITE) ? ALLOYOS_MEMORY_READ_WRITE : ALLOYOS_MEMORY_READ;
            rc = alloyos_syscall3(ALLOYOS_SYS_MEMORY_COMMIT,
                                  (uint64_t)(uintptr_t)addr,
                                  (uint64_t)aligned_len,
                                  mode);
        }
        if (rc < 0) {
            errno = __alloyos_map_kernel_errno(rc);
            return MAP_FAILED;
        }
        return addr;
    }

    int64_t reserved = alloyos_syscall1(ALLOYOS_SYS_MEMORY_RESERVE, (uint64_t)aligned_len);
    if (reserved < 0) {
        errno = __alloyos_map_kernel_errno(reserved);
        return MAP_FAILED;
    }

    if (prot != PROT_NONE) {
        uint64_t mode = (prot & PROT_WRITE) ? ALLOYOS_MEMORY_READ_WRITE : ALLOYOS_MEMORY_READ;
        int64_t committed = alloyos_syscall3(ALLOYOS_SYS_MEMORY_COMMIT,
                                             (uint64_t)reserved,
                                             (uint64_t)aligned_len,
                                             mode);
        if (committed < 0) {
            (void)alloyos_syscall2(ALLOYOS_SYS_MEMORY_RELEASE,
                                   (uint64_t)reserved,
                                   (uint64_t)aligned_len);
            errno = __alloyos_map_kernel_errno(committed);
            return MAP_FAILED;
        }
    }

    return (void*)(uintptr_t)reserved;
}

int mprotect(void* addr, size_t len, int prot)
{
    size_t aligned_len = 0;
    if (!addr || ((uintptr_t)addr & (ALLOYOS_PAGE_SIZE - 1U)) != 0U ||
        align_page_length(len, &aligned_len) != 0) {
        errno = EINVAL;
        return -1;
    }
    if ((prot & ~(PROT_READ | PROT_WRITE | PROT_EXEC)) != 0) {
        errno = EINVAL;
        return -1;
    }
    if ((prot & PROT_EXEC) != 0) {
        errno = ENOTSUP;
        return -1;
    }

    int64_t rc;
    if (prot == PROT_NONE) {
        rc = alloyos_syscall2(ALLOYOS_SYS_MEMORY_DECOMMIT,
                              (uint64_t)(uintptr_t)addr,
                              (uint64_t)aligned_len);
    } else {
        uint64_t mode = (prot & PROT_WRITE) ? ALLOYOS_MEMORY_READ_WRITE : ALLOYOS_MEMORY_READ;
        rc = alloyos_syscall3(ALLOYOS_SYS_MEMORY_COMMIT,
                              (uint64_t)(uintptr_t)addr,
                              (uint64_t)aligned_len,
                              mode);
    }
    if (rc < 0) {
        errno = __alloyos_map_kernel_errno(rc);
        return -1;
    }
    return 0;
}

int munmap(void* addr, size_t len)
{
    size_t aligned_len = 0;
    if (!addr || ((uintptr_t)addr & (ALLOYOS_PAGE_SIZE - 1U)) != 0U ||
        align_page_length(len, &aligned_len) != 0) {
        errno = EINVAL;
        return -1;
    }
    int64_t rc = alloyos_syscall2(ALLOYOS_SYS_MEMORY_RELEASE,
                                  (uint64_t)(uintptr_t)addr,
                                  (uint64_t)aligned_len);
    if (rc < 0) {
        errno = __alloyos_map_kernel_errno(rc);
        return -1;
    }
    return 0;
}

int madvise(void* addr, size_t len, int advice)
{
    size_t aligned_len = 0;
    if (!addr || ((uintptr_t)addr & (ALLOYOS_PAGE_SIZE - 1U)) != 0U ||
        align_page_length(len, &aligned_len) != 0) {
        errno = EINVAL;
        return -1;
    }
    if (advice == MADV_DONTNEED || advice == MADV_FREE) {
        int64_t rc = alloyos_syscall2(ALLOYOS_SYS_MEMORY_DECOMMIT,
                                      (uint64_t)(uintptr_t)addr,
                                      (uint64_t)aligned_len);
        if (rc < 0) {
            errno = __alloyos_map_kernel_errno(rc);
            return -1;
        }
        rc = alloyos_syscall3(ALLOYOS_SYS_MEMORY_COMMIT,
                              (uint64_t)(uintptr_t)addr,
                              (uint64_t)aligned_len,
                              ALLOYOS_MEMORY_READ_WRITE);
        if (rc < 0) {
            errno = __alloyos_map_kernel_errno(rc);
            return -1;
        }
        return 0;
    }
    if (advice == MADV_NORMAL || advice == MADV_RANDOM ||
        advice == MADV_SEQUENTIAL || advice == MADV_WILLNEED) {
        return 0;
    }
    errno = EINVAL;
    return -1;
}

int posix_madvise(void* addr, size_t len, int advice)
{
    if (madvise(addr, len, advice) != 0) {
        return errno;
    }
    return 0;
}

int msync(void* addr, size_t len, int flags)
{
    /* Mappings are anonymous/private in alloyOS; there is no backing store to flush. */
    (void)addr; (void)len;
    if (flags & ~(MS_ASYNC | MS_SYNC | MS_INVALIDATE)) {
        errno = EINVAL;
        return -1;
    }
    return 0;
}
