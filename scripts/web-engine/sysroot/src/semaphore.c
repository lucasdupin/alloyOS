#include <errno.h>
#include <semaphore.h>

int sem_init(sem_t* sem, int pshared, unsigned int value)
{
    if (!sem || value > SEM_VALUE_MAX) { errno = EINVAL; return -1; }
    if (pshared) { errno = ENOSYS; return -1; }
    sem->value = value;
    if (pthread_mutex_init(&sem->mutex, NULL) != 0) { errno = EINVAL; return -1; }
    if (pthread_cond_init(&sem->cond, NULL) != 0) { errno = EINVAL; return -1; }
    return 0;
}

int sem_destroy(sem_t* sem)
{
    if (!sem) { errno = EINVAL; return -1; }
    pthread_cond_destroy(&sem->cond);
    pthread_mutex_destroy(&sem->mutex);
    return 0;
}

int sem_post(sem_t* sem)
{
    if (!sem) { errno = EINVAL; return -1; }
    pthread_mutex_lock(&sem->mutex);
    if (sem->value == SEM_VALUE_MAX) {
        pthread_mutex_unlock(&sem->mutex);
        errno = EOVERFLOW;
        return -1;
    }
    sem->value++;
    pthread_cond_signal(&sem->cond);
    pthread_mutex_unlock(&sem->mutex);
    return 0;
}

int sem_wait(sem_t* sem)
{
    if (!sem) { errno = EINVAL; return -1; }
    pthread_mutex_lock(&sem->mutex);
    while (sem->value == 0)
        pthread_cond_wait(&sem->cond, &sem->mutex);
    sem->value--;
    pthread_mutex_unlock(&sem->mutex);
    return 0;
}

int sem_trywait(sem_t* sem)
{
    if (!sem) { errno = EINVAL; return -1; }
    int rc = 0;
    pthread_mutex_lock(&sem->mutex);
    if (sem->value == 0) { errno = EAGAIN; rc = -1; }
    else sem->value--;
    pthread_mutex_unlock(&sem->mutex);
    return rc;
}

int sem_timedwait(sem_t* sem, const struct timespec* abs_timeout)
{
    if (!sem || !abs_timeout) { errno = EINVAL; return -1; }
    pthread_mutex_lock(&sem->mutex);
    while (sem->value == 0) {
        int rc = pthread_cond_timedwait(&sem->cond, &sem->mutex, abs_timeout);
        if (rc != 0 && sem->value == 0) {
            pthread_mutex_unlock(&sem->mutex);
            errno = rc;
            return -1;
        }
    }
    sem->value--;
    pthread_mutex_unlock(&sem->mutex);
    return 0;
}

int sem_getvalue(sem_t* sem, int* sval)
{
    if (!sem || !sval) { errno = EINVAL; return -1; }
    pthread_mutex_lock(&sem->mutex);
    *sval = (int)sem->value;
    pthread_mutex_unlock(&sem->mutex);
    return 0;
}
