#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <limits.h>
#include <assert.h>
#include <stdbool.h>

#include "printf-ext.h"

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
#define LLU(a) ((llu)a)
#define LP(_a, _b) ((struct llu_pair){(_a), (_b)})
#define LLU_EQ(_a, _b) (((_a).a == (_b).a) && ((_a).b == (_b).b))
#define LLU_PAIR_EXP(_a) (_a).a, (_a).b
#define LLU_FMT "0x%llx 0x%llx"

#define TEST 1
#if TEST
static unsigned long test__error_ct = 0;
#define test_eq_xp(a, b) test_eq_fmt_exp((a), (b), LLU_FMT, LLU_PAIR_EXP, LLU_EQ)
#define test_eq_x(a, b) test_eq_fmt_exp((a), (b), "0x%llx", LLU, EQ)
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

static uint8_t fls_nz(llu num)
{
	return next_set_bit_nz(num, BITS_IN(num));
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

/*
 * extend the numerator with W zeros, where W is (fls(denominator) - 1)
 */
static struct llu_pair poly_div_base(llu numerator, llu denominator, int8_t extend)
{
	assert(numerator != 0);

	int8_t denom_bits = fls(denominator);
	int8_t numer_bits = fls_nz(numerator);
	/* How far the denominator is currently shifted left */
	llu shifted_denom = denominator << (numer_bits - denom_bits);
	int8_t denom_shift = numer_bits - denom_bits;
	llu quotient = 0;

	if (numer_bits < denom_bits)
		return (struct llu_pair) { quotient, numerator };

	printf("> n 0x%04llx d 0x%04llx e 0x%02x\n", numerator, denominator, extend);

	int8_t next_bit = numer_bits;
	int8_t next_shift_ct = 0;
	for (;;) {
		quotient <<= next_shift_ct;
		quotient |= 1;

		numerator ^= shifted_denom;

		int8_t prev_bit = next_bit;
		next_bit = fls(numerator);
		next_shift_ct = prev_bit - next_bit;

		if (next_shift_ct > (denom_shift + extend)) {
			quotient <<= (denom_shift + extend);
			return (struct llu_pair) { quotient, numerator };
		}

		denom_shift -= next_shift_ct;
		numerator <<= next_shift_ct;

		printf(" N 0x%04llx ds 0x%02x nsc 0x%02x\n", numerator, denom_shift, next_shift_ct);
	}
}

#if 0
struct bit_seq {
	llu data;
	int8_t bits; /* # of bits */

	/* calculated */
	llu top_bit_mask;
	llu mask;
};

static bool bit_seq_shift_left(struct bit_seq *b)
{
	bool bit = b->data & b->top_bit_mask;
	b->data = (b->data << 1) & b->mask;
	return bit;
}

static void bit_seq_xor(struct bit_seq *a, struct bit_seq *b)
{

}
#endif

/* Based on http://www.zlib.net/crc_v3.txt "SIMPLE" method.
 * -----
 * Load the register with zero bits.
 * Augment the message by appending W zero bits to the end of it.
 * While (more message bits)
 *    Begin
 *    Shift the register left by one bit, reading the next bit of the
 *       augmented message into register bit position 0.
 *    If (a 1 bit popped out of the register during step 3)
 *       Register = Register XOR Poly.
 *    End
 * The register now contains the remainder.
 */
static llu crc_update_simple(llu msg, int8_t msg_bits,
		llu rem, llu poly, int8_t poly_bits)
{
	/* Load the register with zero bits. */
	llu reg = rem;

	llu msg_bit_mask = 1 << (msg_bits - 1);
	llu reg_mask = (1 << (poly_bits - 1)) - 1;
	llu reg_bit_mask = 1 << (poly_bits - 2);

	/* While (more message bits)
	 * (the "--poly_bits" serves to extend the message by W bits) */
	while(msg_bits || (--poly_bits)) {
		bool bit = !!(reg & reg_bit_mask);
		reg <<= 1;
		reg |= !!(msg & msg_bit_mask);

		msg_bits --;
		msg_bit_mask >>= 1;

		/* if a 1 bit poped out, xor reg with poly */
		if (bit) {
			reg ^= poly;
			reg &= reg_mask;
		}
	}

	return reg;
}

static llu poly_mult(llu a, llu b)
{
	llu r = 0;
	while (a) {
		r ^= b;
		b <<= 1;
		a--;
	}

	return r;
}

static llu poly_undiv_(struct llu_pair p, llu m)
{
	return poly_mult(p.a, m) ^ p.b;
}

/* like the below poly_div(), but instead of shifting the denominator down,
 * shift the numerator up */
static struct llu_pair poly_div_shift_numer(llu numerator, llu denominator)
{
	assert(numerator != 0);

#define PS(msg) printf(">> %-17s | Q %#llx N %#llx nsc 0x%x\n", msg, quotient, numerator, next_shift_ct)

	printf("--\n");

	int8_t denom_bits = fls(denominator);
	int8_t numer_bits = fls_nz(numerator);
	int8_t denom_shift = numer_bits - denom_bits;
	llu quotient = 0;
	int8_t next_shift_ct = 0;

	if (numer_bits < denom_bits)
		return (struct llu_pair) { quotient, numerator };

	/* align it's first bit with the numerator's first bit */
	PS("- denom shift");
	denominator <<= denom_shift;
	PS("+ denom shift");

	for (;;) {
		PS("- quotient update");
		quotient <<= next_shift_ct;
		quotient |= 1;
		PS("+ quotient update");

		numerator ^= denominator;
		next_shift_ct = numer_bits - fls(numerator);

		PS("- exit check");
		if (next_shift_ct > denom_shift) {
			printf("ds = %#x\n", denom_shift);
			return (struct llu_pair) {
				//quotient << (next_shift_ct - 1), // works
				//numerator >> (numer_bits - denom_bits - 1) //works with #1, broken #2
				quotient << (denom_shift + 1),
				numerator >> (numer_bits - denom_bits + denom_shift)
			};
		}

		PS("- shift updates");
		denom_shift -= next_shift_ct;
		numerator <<= next_shift_ct;
		PS("+ shift updates");
	}
}
#if 0
/* use fls() and shift the denominator along to perform polynomial division.
 * Assumes that all bits in the poly fit into numerator/denominator */
static struct llu_pair poly_div(llu numerator, llu denominator)
{
	assert(numerator != 0);

	int8_t denom_bits = fls(denominator);
	int8_t numer_bits = fls_nz(numerator);
	/* How far the denominator is currently shifted left */
	llu shifted_denom = denominator << (numer_bits - denom_bits);
	int8_t denom_shift = numer_bits - denom_bits;
	llu quotient = 0;

	if (numer_bits < denom_bits)
		return (struct llu_pair) { quotient, numerator };

	int8_t next_shift_ct = 0;
	int8_t next_bit = numer_bits;
	for (;;) {
		quotient <<= next_shift_ct;
		quotient |= 1;

		numerator ^= shifted_denom;

		int8_t prev_bit = next_bit;
		next_bit = fls(numerator);
		next_shift_ct = prev_bit - next_bit;

		if (next_shift_ct > denom_shift) {
			quotient <<= denom_shift;
			return (struct llu_pair) { quotient, numerator };
		}

		denom_shift -= next_shift_ct;
		shifted_denom >>= next_shift_ct;
		assert(shifted_denom);
	}
}
#else
static struct llu_pair poly_div(llu numerator, llu denominator)
{
	return poly_div_base(numerator, denominator, 0);
}
#endif

static llu crc_update(llu msg, llu poly)
{
	printf("EXTEND 0x%x\n", fls(poly) - 1);
	struct llu_pair p = poly_div_base(msg, poly, fls(poly) - 1);
	printf("A 0x%llx\n", p.a);
	return p.b;
}

int main(int argc, char **argv)
{
	register_printf_b();

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
	test_eq_x(0xf000u, align_left(0xf, 16));
	test_eq_xp(LP(778, 14), poly_div(0x35b0, 0x13));
	test_eq_xp(LP(0, 0x10), poly_div(0x10, 0x10 << 1));
	test_eq_xp(LP(778, 14), poly_div_base(0x35b, 0x13, 4));
	test_eq_x(14u, crc_update(0x35b, 0x13));
	test_eq_x(14u, crc_update_simple(0x35b, fls(0x35b), 0, 0x13, fls(0x13)));
	test_eq_xp(LP(778, 14), poly_div_shift_numer(0x35b0, 0x13));
	test_eq_xp(LP(806, 0xf), poly_div_shift_numer(0x35b0, (0x12 << 1) | 1));
	test_done();

	return 0;
}
