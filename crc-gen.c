#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <limits.h>
#include <assert.h>


#define BUILD_BUG_ON_INVALID(e) ((void)(sizeof((long)(e))))
__attribute__((format(printf,1,2)))
static inline void printf_check_fmt(const char *fmt __attribute__((unused)), ...)
{}
typedef unsigned long long llu;
#define UNIT(x) x
#define EQ(a, b) ((a) == (b))
struct llu_pair {
	llu a, b;
};
#define LLU_EQ(_a, _b) (((_a).a == (_b).a) && ((_a).b == (_b).b))
#define LLU_PAIR_EXP(_a) (_a).a, (_a).b
#define LLU_FMT "0x%llx 0x%llx"

#define TEST 1
#if TEST
static unsigned long test__error_ct = 0;
#define test_eq_xp(a, b) test_eq_fmt_exp((a), (b), LLU_FMT, LLU_PAIR_EXP, LLU_EQ)
#define test_eq_x(a, b) test_eq_fmt((llu)(a), (llu)(b), "0x%llx")
#define test_eq_fmt(a, b, fmt) test_eq_fmt_exp(a, b, fmt, UNIT, EQ)
#define test_eq_fmt_exp(a, b, fmt, exp, eq) do {			\
	typeof(a) __test_eq_a = (a);						\
	typeof(b) __test_eq_b = (b);						\
	if (!eq((__test_eq_a), (__test_eq_b))) {				\
		fprintf(stderr, "TEST FAILURE: %s ("fmt") != %s ("fmt")\n",	\
				#a, exp(__test_eq_a), #b, exp(__test_eq_b));	\
		test__error_ct ++;						\
	}									\
} while(0)
#define test_done() do {							\
	if (test__error_ct > 0)	{						\
		fprintf(stderr, "TESTS FAILED: %lu, exiting\n", test__error_ct);	\
		exit(1);							\
	}									\
} while(0)
#else
#define test_eq_x(a, b) BUILD_BUG_ON_INVALID((a) == (b))
#define test_eq_fmt(a, a_fmt, b, b_fmt) do {	\
	BUILD_BUG_ON_INVALID((a) == (b));	\
	printf_check_fmt(a_fmt b_fmt, a, b);	\
} while(0)
#define test(x) BUILD_BUG_ON_INVALID(e)
#define test_done() do {} while(0)
#endif

static void usage(const char *prgm)
{
	FILE *f = stdout;
	fprintf(f, "usage: %s <crc-polynomial>\n", prgm);
	exit(EXIT_FAILURE);
}
#define USAGE usage(argc?argv[0]:"crc-gen")
#define BITS_IN(type) (CHAR_BIT * sizeof(type))

/**
 * fls_next(num, bit_idx) - find the next set bit, searching from the left
 * "find last set next"
 *
 * num: number to search for bits
 * bit_idx: The highest 1-indexed bit to examine (or the highest 0-indexed bit
 *          + 1)
 */
static uint8_t next_set_bit(llu num, uint8_t bit_idx)
{
	for (;;) {
		if (bit_idx == 0)
			return 0;
		if (num & (1llu << (bit_idx - 1llu)))
			return bit_idx;
		bit_idx --;
	}
}

static uint8_t next_set_bit_nz(llu num, uint8_t bit_idx)
{
	for (;;) {
		if (num & (1llu << (bit_idx - 1llu)))
			return bit_idx;
		bit_idx --;
	}
}

/**
 * fls(num) - find the last (most significant) set bit
 *
 * Returns (the left shift of the found set bit) + 1
 *     If no set bit is found, returns 0.
 */
static uint8_t fls(llu num)
{
	return next_set_bit(num, BITS_IN(num));
}

static llu align_left(llu num, uint8_t bit_space)
{
	return num << (bit_space - next_set_bit(num, bit_space));
}

/*
            1100001010
       _______________
10011 ) 11010110110000
        10011,,.,,....
        -----,,.,,....
         10011,.,,....
         10011,.,,....
         -----,.,,....
          00001.,,....
          00000.,,....
          -----.,,....
           00010,,....
           00000,,....
           -----,,....
            00101,....
            00000,....
            -----,....
             01011....
             00000....
             -----....
              10110...
              10011...
              -----...
               01010..
               00000..
               -----..
                10100.
                10011.
                -----.
                 01110
                 00000
                 -----
                  1110 = Remainder = 14 = 0xe

	Quotient = 1100001010 = 778 = 0x30a
	Numerator (dividend) = 11010110110000 = 0x35b0 = 13744
	Denominator (divisor) = 10011 = 0x13 = 19

	SD	N		Q
	10011	11010110110000	0
		10011						N ^= SD
		     123456789
				1				update Q
								check complete?
								Update SD

		01001110110000					N ^= SD
		 10011		(>>1)
				11	(1 << 1 | 1)		update Q

		00000010110000
		      10011	(>>5)
				1100001 (11 << 5 | 1)

		00000000101000
		        10011	(>>2)
				110000101 (1100001 << 2 | 1)

		00000000001110
				1100001010 (110000101 << 1)

 */

static struct llu_pair poly_div(llu numerator, llu denominator)
{
	uint8_t shift_ct = 0, next_bit, prev_shift_ct;
	uint8_t pos = BITS_IN(numerator) + 1;
	uint8_t denom_bits = fls(denominator);
	llu quotient = 0;
	assert(numerator != 0);
	/* essentially, flsll() */
	pos = next_set_bit_nz(numerator, pos);
	/* denom might not even fit once */
	if (pos < denom_bits) {
		printf("1 pos = 0x%x < denom_bits = 0x%x\n", pos, denom_bits);
		return (struct llu_pair) { 0, numerator };
	}

	uint8_t denom_shift_rem = pos - denom_bits;
	llu shifted_denom = denominator << denom_shift_rem;

	for (;;) {
		printf("N 0x%llx ^ 0x%llx = 0x%llx\n", numerator, shifted_denom, numerator ^ shifted_denom);
		numerator ^= shifted_denom;

		prev_shift_ct = shift_ct;
		next_bit = next_set_bit(numerator, pos);
		shift_ct = pos - next_bit;
		denom_shift_rem -= shift_ct;

		printf("DENUM SHIFT REM 0x%x < 0x%x\n", denom_shift_rem, denom_bits);
		if (denom_shift_rem < denom_bits) {
			quotient <<= denom_shift_rem;
			return (struct llu_pair) { quotient, numerator };
		}
		printf("Q 0x%llx << %d | 1 = 0x%llx\n", quotient, prev_shift_ct, (quotient << prev_shift_ct) | 1);
		quotient <<= prev_shift_ct;
		quotient |= 1;

		printf("SHIFT DENOM = 0x%llx (%d)\n", shifted_denom, shift_ct);
		shifted_denom >>= shift_ct;
		assert(shifted_denom);
	}
}

int main(int argc, char **argv)
{
	if (argc != 2)
		USAGE;

	llu poly;
	int r = sscanf(argv[1], "%lli", &poly);
	if (r != 1) {
		printf("Argument \"%s\" is not number we can use.\n", argv[1]);
		exit(EXIT_FAILURE);
	}

	test_eq_x(1, next_set_bit(0x1, 10));
	test_eq_x(5, next_set_bit(0x10, 10));
	test_eq_x(4, next_set_bit(0xf, 10));
	test_eq_x(4, next_set_bit(0xf, 33));
	test_eq_x(4, next_set_bit(0xf, BITS_IN(0xf) + 1));
	test_eq_x(1, fls(1));
	test_eq_x(5, fls(0x10));
	test_eq_x(4, fls(0xf));
	test_eq_x(0xf000, align_left(0xf, 16));
	test_eq_xp(((struct llu_pair){ 778, 14 }), poly_div(0x35b0, 0x13));
	test_done();


	return 0;
}
