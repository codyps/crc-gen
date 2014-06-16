#include <printf.h>
#include <limits.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>

#include "printf-ext.h"

static int print_b(FILE *f,
	const struct printf_info *info,
	const void *const *args)
{
	unsigned long long n;
	if (info->is_char)
		n = *((const unsigned *)args[0]);
	else if (info->is_short)
		n = *((const unsigned short *)args[0]);
	else if (info->is_long)
		n = *((const unsigned long *)args[0]);
	else if (info->is_long_double)
		n = *((const unsigned long long *)args[0]);
	else
		n = *((const unsigned *)args[0]);

	int used = fprintf(f, "0b");

	int8_t bit;
	bool started = false;
	for (bit = sizeof(n) * CHAR_BIT;
			bit;
			bit--) {
		bool v = !!(n & 1 << (bit - 1));
		if (v || started || (bit < info->width)) {
			started = true;
			used += fprintf(f, "%d", v);
		}
	}

	return used;
}

static int arginfo_size_b(const struct printf_info *info,
		size_t n, int *argtypes, int *size)
{
	if (n > 0) {
		if (info->is_char)
			argtypes[0] = PA_CHAR;
		else if (info->is_short)
			argtypes[0] = PA_INT | PA_FLAG_SHORT;
		else if (info->is_long)
			argtypes[0] = PA_INT | PA_FLAG_LONG;
		else if (info->is_long_double)
			argtypes[0] = PA_INT | PA_FLAG_LONG_LONG;
		else
			argtypes[0] = PA_INT;
	}

	return 1;
}

int register_printf_b(void)
{
	return register_printf_specifier('b',
		print_b, arginfo_size_b);
}
