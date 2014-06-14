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

/* fls_next() */
static uint8_t next_set_bit(llu num, uint8_t bit_idx)
{
	for (;;) {
		if (bit_idx == 0)
			return 0;
		if (num & (1 << (bit_idx - 1)))
			return bit_idx;
		bit_idx --;
	}
}

static uint8_t next_set_bit_nz(llu num, uint8_t bit_idx)
{
	for (;;) {
		if (num & (1 << (bit_idx - 1)))
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
	return next_set_bit(num, BITS_IN(num) + 1);
}

static llu align_left(llu num, uint8_t bit_space)
{
	return num << (bit_space - next_set_bit(num, bit_space));
}

static struct llu_pair poly_div(llu numerator, llu denominator)
{
	uint8_t next_pos;
	uint8_t pos = BITS_IN(numerator) + 1;
	uint8_t denom_bits = fls(denominator);
	llu quotient = 1;
	assert(numerator != 0);
	/* essentially, flsll() */
	pos = next_set_bit_nz(numerator, pos);
	/* denom might not even fit once */
	if (pos < denom_bits)
		return (struct llu_pair) { 0, numerator };
	llu shifted_denom = align_left(denominator, pos);
	for (;;) {
		numerator ^= shifted_denom;
		quotient ++;
		next_pos = next_set_bit(numerator, pos);
		if (next_pos < denom_bits)
			return (struct llu_pair) { quotient, numerator };
		shifted_denom = shifted_denom >> (pos - next_pos);
	}
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
 */

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
	test_eq_x(0xf000, align_left(0xf, 16));
	test_eq_xp(((struct llu_pair){ 778, 14 }), poly_div(0x35b0, 0x13));
	test_done();


	return 0;
}
