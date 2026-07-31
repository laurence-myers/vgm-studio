/* A small capturing printf for the vgmtools optimisers on wasm32-unknown-unknown
 * -- C, because variadics cannot come from Rust.
 *
 * `printf` appends to a capped ring so the host can quote a failing tool's last
 * words (e.g. vgm_sro's "RF5Cxx Memory Writes aren't supported!"); sprintf and
 * snprintf format into caller buffers. It is a subset -- %d %i %u %x %X %c %s
 * %f %%, the '-'/'0' flags, width, .precision, and the l/ll length modifiers --
 * which is everything the three tools reach for. **None of it affects the bytes
 * a tool writes**, so it can never break the byte-parity gate; its only job is a
 * readable error tail. See OPTIMIZER-WASM-PLAN.md, Re-evaluation correction D.
 */

#include <stddef.h>
#include <stdarg.h>

/* --- the log ring ------------------------------------------------------ */

#define LOG_CAP 4096
static char g_log[LOG_CAP];
static size_t g_log_len;

const char* vgmt_log_ptr(void)
{
	return g_log;
}

unsigned int vgmt_log_len(void)
{
	return (unsigned int)g_log_len;
}

/* Keep the last LOG_CAP bytes: errors are the last thing a tool prints, so the
 * tail is what matters. */
static void log_append(const char* text, size_t n)
{
	size_t i;
	if (n >= LOG_CAP)
	{
		for (i = 0; i < LOG_CAP; i++)
			g_log[i] = text[n - LOG_CAP + i];
		g_log_len = LOG_CAP;
		return;
	}
	if (g_log_len + n > LOG_CAP)
	{
		size_t drop = g_log_len + n - LOG_CAP;
		for (i = 0; i + drop < g_log_len; i++)
			g_log[i] = g_log[i + drop];
		g_log_len -= drop;
	}
	for (i = 0; i < n; i++)
		g_log[g_log_len + i] = text[i];
	g_log_len += n;
}

/* --- a bounded output sink --------------------------------------------- */

typedef struct {
	char* buf;    /* NULL to count only */
	size_t cap;   /* bytes available including the NUL */
	size_t count; /* chars that would be written (may exceed cap) */
} sink;

static void put(sink* s, char c)
{
	if (s->buf != NULL && s->cap != 0 && s->count < s->cap - 1)
		s->buf[s->count] = c;
	s->count++;
}

static void put_pad(sink* s, char c, int n)
{
	while (n-- > 0)
		put(s, c);
}

/* Emit `text[0..len)` in a `width` field, left- or right-justified. */
static void put_field(sink* s, const char* text, int len, int width, int left)
{
	int pad = width - len;
	int i;
	if (pad < 0)
		pad = 0;
	if (!left)
		put_pad(s, ' ', pad);
	for (i = 0; i < len; i++)
		put(s, text[i]);
	if (left)
		put_pad(s, ' ', pad);
}

/* Render an unsigned value in `base` (10 or 16) into `out` (reversed then
 * flipped), returning the digit count. */
static int u_to_str(unsigned long long value, int base, int upper, char* out)
{
	const char* digits = upper ? "0123456789ABCDEF" : "0123456789abcdef";
	char tmp[24];
	int n = 0, i;
	do {
		tmp[n++] = digits[value % (unsigned)base];
		value /= (unsigned)base;
	} while (value != 0);
	for (i = 0; i < n; i++)
		out[i] = tmp[n - 1 - i];
	return n;
}

/* An integer conversion with sign, width, zero-pad and left-justify. */
static void put_int(sink* s, int negative, unsigned long long mag, int base,
	int upper, int width, int zero, int left)
{
	char body[24];
	int len = u_to_str(mag, base, upper, body);
	int sign = negative ? 1 : 0;
	int pad = width - len - sign;
	if (pad < 0)
		pad = 0;
	if (zero && !left)
	{
		if (sign)
			put(s, '-');
		put_pad(s, '0', pad);
		put_field(s, body, len, 0, 0);
	}
	else if (!left)
	{
		put_pad(s, ' ', pad);
		if (sign)
			put(s, '-');
		put_field(s, body, len, 0, 0);
	}
	else
	{
		if (sign)
			put(s, '-');
		put_field(s, body, len, 0, 0);
		put_pad(s, ' ', pad);
	}
}

/* Fixed-point %f. The tools only print small percentages, so magnitudes stay
 * well inside i64 after scaling; larger values simply lose their fraction. */
static void put_float(sink* s, double value, int prec, int width, int zero, int left)
{
	char body[64];
	int len = 0, i;
	int negative = 0;
	unsigned long long scale = 1;
	unsigned long long scaled, ipart, frac;
	char fracbuf[24];
	int fraclen;

	if (prec < 0)
		prec = 6;
	if (prec > 9)
		prec = 9;
	if (value < 0)
	{
		negative = 1;
		value = -value;
	}
	for (i = 0; i < prec; i++)
		scale *= 10;
	/* round half up */
	scaled = (unsigned long long)(value * (double)scale + 0.5);
	ipart = scaled / scale;
	frac = scaled % scale;

	len += u_to_str(ipart, 10, 0, body + len);
	if (prec > 0)
	{
		body[len++] = '.';
		fraclen = u_to_str(frac, 10, 0, fracbuf);
		/* left-pad the fraction with zeros to `prec` digits */
		for (i = 0; i < prec - fraclen; i++)
			body[len++] = '0';
		for (i = 0; i < fraclen; i++)
			body[len++] = fracbuf[i];
	}

	{
		int sign = negative ? 1 : 0;
		int pad = width - len - sign;
		if (pad < 0)
			pad = 0;
		if (zero && !left)
		{
			if (sign)
				put(s, '-');
			put_pad(s, '0', pad);
			put_field(s, body, len, 0, 0);
		}
		else if (!left)
		{
			put_pad(s, ' ', pad);
			if (sign)
				put(s, '-');
			put_field(s, body, len, 0, 0);
		}
		else
		{
			if (sign)
				put(s, '-');
			put_field(s, body, len, 0, 0);
			put_pad(s, ' ', pad);
		}
	}
}

static int str_len_capped(const char* s, int cap)
{
	int n = 0;
	if (s == NULL)
		return 0;
	while (s[n] != '\0' && (cap < 0 || n < cap))
		n++;
	return n;
}

static size_t format(char* out, size_t cap, const char* fmt, va_list ap)
{
	sink s;
	s.buf = out;
	s.cap = cap;
	s.count = 0;

	while (*fmt)
	{
		int left = 0, zero = 0, width = 0, prec = -1, longmod = 0;

		if (*fmt != '%')
		{
			put(&s, *fmt++);
			continue;
		}
		fmt++; /* past '%' */

		/* flags */
		for (;;)
		{
			if (*fmt == '-') { left = 1; fmt++; }
			else if (*fmt == '0') { zero = 1; fmt++; }
			else if (*fmt == '+' || *fmt == ' ' || *fmt == '#') { fmt++; }
			else break;
		}
		/* width */
		while (*fmt >= '0' && *fmt <= '9')
			width = width * 10 + (*fmt++ - '0');
		/* precision */
		if (*fmt == '.')
		{
			fmt++;
			prec = 0;
			while (*fmt >= '0' && *fmt <= '9')
				prec = prec * 10 + (*fmt++ - '0');
		}
		/* length modifier */
		while (*fmt == 'l' || *fmt == 'h' || *fmt == 'z')
		{
			if (*fmt == 'l')
				longmod++;
			fmt++;
		}

		switch (*fmt)
		{
		case 'd':
		case 'i':
		{
			long long v = (longmod >= 2) ? va_arg(ap, long long)
				: (longmod == 1) ? (long long)va_arg(ap, long)
				: (long long)va_arg(ap, int);
			int neg = v < 0;
			unsigned long long mag = neg ? (unsigned long long)(-(v + 1)) + 1ull
				: (unsigned long long)v;
			put_int(&s, neg, mag, 10, 0, width, zero, left);
			break;
		}
		case 'u':
		{
			unsigned long long v = (longmod >= 2) ? va_arg(ap, unsigned long long)
				: (longmod == 1) ? (unsigned long long)va_arg(ap, unsigned long)
				: (unsigned long long)va_arg(ap, unsigned int);
			put_int(&s, 0, v, 10, 0, width, zero, left);
			break;
		}
		case 'x':
		case 'X':
		{
			unsigned long long v = (longmod >= 2) ? va_arg(ap, unsigned long long)
				: (longmod == 1) ? (unsigned long long)va_arg(ap, unsigned long)
				: (unsigned long long)va_arg(ap, unsigned int);
			put_int(&s, 0, v, 16, (*fmt == 'X'), width, zero, left);
			break;
		}
		case 'c':
		{
			char c = (char)va_arg(ap, int);
			put_field(&s, &c, 1, width, left);
			break;
		}
		case 's':
		{
			const char* str = va_arg(ap, const char*);
			int len = str_len_capped(str, prec);
			if (str == NULL)
				str = "(null)", len = str_len_capped(str, prec);
			put_field(&s, str, len, width, left);
			break;
		}
		case 'f':
		case 'F':
			put_float(&s, va_arg(ap, double), prec, width, zero, left);
			break;
		case '%':
			put(&s, '%');
			break;
		case '\0':
			/* trailing '%' -- emit it literally and stop */
			put(&s, '%');
			goto done;
		default:
			/* unknown: echo it verbatim so nothing is silently swallowed */
			put(&s, '%');
			put(&s, *fmt);
			break;
		}
		fmt++;
	}
done:
	if (s.buf != NULL && s.cap != 0)
		s.buf[s.count < s.cap ? s.count : s.cap - 1] = '\0';
	return s.count;
}

/* --- the public surface ------------------------------------------------ */

int vsnprintf(char* str, size_t size, const char* fmt, va_list ap)
{
	return (int)format(str, size, fmt, ap);
}

int snprintf(char* str, size_t size, const char* fmt, ...)
{
	va_list ap;
	int n;
	va_start(ap, fmt);
	n = (int)format(str, size, fmt, ap);
	va_end(ap);
	return n;
}

int sprintf(char* str, const char* fmt, ...)
{
	va_list ap;
	int n;
	va_start(ap, fmt);
	/* No size from the caller; the only sprintf user (common.h's unused
	 * PrintMinSec) writes a handful of chars. Cap generously. */
	n = (int)format(str, (size_t)0x7fffffff, fmt, ap);
	va_end(ap);
	return n;
}

int printf(const char* fmt, ...)
{
	char tmp[512];
	va_list ap;
	size_t n;
	va_start(ap, fmt);
	n = format(tmp, sizeof(tmp), fmt, ap);
	va_end(ap);
	log_append(tmp, n < sizeof(tmp) - 1 ? n : sizeof(tmp) - 1);
	return (int)n;
}
