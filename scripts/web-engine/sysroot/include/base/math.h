#ifndef _ALLOYOS_MATH_H
#define _ALLOYOS_MATH_H

#ifdef __cplusplus
extern "C" {
#endif

#define NAN       (__builtin_nanf(""))
#define INFINITY  (__builtin_inff())

double fabs(double x);
float fabsf(float x);
double sqrt(double x);
float sqrtf(float x);
double floor(double x);
double ceil(double x);
double trunc(double x);
double round(double x);
double sin(double x);
double cos(double x);

#ifdef __cplusplus
}
#endif

#endif /* _ALLOYOS_MATH_H */
