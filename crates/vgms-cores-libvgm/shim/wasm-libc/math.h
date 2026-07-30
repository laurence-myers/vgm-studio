/* A freestanding stand-in for libc's <math.h>, for wasm32-unknown-unknown.
 *
 * The cores use doubles to *build tables* (volume curves, pan laws), not in
 * their sample loops, so a plain-IEEE software implementation is fine: the
 * symbols come from `src/wasm_libc.rs`, forwarding to the pure-Rust `libm`
 * crate. floor/ceil/fabs/sqrt lower to wasm instructions where clang can.
 */

#pragma once

double pow(double base, double exponent);
double sqrt(double value);
double exp(double value);
double log(double value);
double log2(double value);
double log10(double value);
double sin(double value);
double cos(double value);
double tan(double value);
double asin(double value);
double acos(double value);
double atan(double value);
double atan2(double y, double x);
double sinh(double value);
double cosh(double value);
double tanh(double value);
double floor(double value);
double ceil(double value);
double fabs(double value);
double fmod(double a, double b);
double ldexp(double value, int exponent);

float powf(float base, float exponent);
float sqrtf(float value);
float expf(float value);
float logf(float value);
float sinf(float value);
float cosf(float value);
float floorf(float value);
float fabsf(float value);
float fmodf(float a, float b);

#ifndef M_PI
#define M_PI 3.14159265358979323846
#endif
#ifndef M_SQRT2
#define M_SQRT2 1.41421356237309504880
#endif
