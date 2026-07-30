/* wasm32-unknown-unknown stubs for the printf family -- C because variadics
 * cannot be provided from Rust.
 *
 * `logging.c` formats a message and hands it to a registered callback; this
 * crate never registers one (the Rust side logs through `log::` instead), so
 * the formatted text is write-only. Truncating it to the empty string keeps
 * the link honest without dragging a formatter into the module.
 */

#include <stdarg.h>
#include <stddef.h>

int vsnprintf(char *buffer, size_t size, const char *format, va_list args) {
	(void)format;
	(void)args;
	if (buffer != NULL && size > 0)
		buffer[0] = '\0';
	return 0;
}

int snprintf(char *buffer, size_t size, const char *format, ...) {
	(void)format;
	if (buffer != NULL && size > 0)
		buffer[0] = '\0';
	return 0;
}
