
#define TEST 1
#define TEST_FILE stdout

#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <limits.h>
#include <assert.h>
#include <stdbool.h>
#include <penny/penny.h>
#include <penny/print.h>
#include <penny/test.h>
#include <penny/type_info.h>
#include <penny/bits.h>

#include <ccan/array_size/array_size.h>
#include <ccan/darray/darray.h>

#include "printf-ext.h"

struct llu_pair {
	llu a, b;
};
#define LLU(a) ((llu)a)
#define LP(_a, _b) ((struct llu_pair){(_a), (_b)})
#define LLU_EQ(_a, _b) (((_a).a == (_b).a) && ((_a).b == (_b).b))
#define LLU_PAIR_EXP(_a) (_a).a, (_a).b
#define LLU_FMT "0x%llx 0x%llx"

#define B(x) do { printf("0b"); print_bits_llu(x, stdout); printf("\n"); } while (0)
#define D(x, fmt) printf(#x " = %#" #fmt "\n", x)

static void usage(const char *prgm)
{
	FILE *f = stdout;
	fprintf(f, "usage: %s <crc-polynomial>\n", prgm);
	exit(EXIT_FAILURE);
}
#define USAGE usage(argc?argv[0]:"crc-gen")

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

enum num_type {
	NT_KNOWN,
	NT_SYM,
};

struct num;
/* num_bit *-> num */
struct num_bit {
	struct num *num;
	llu bit_idx;
};
struct num {
	enum num_type nt;
	llu val;
	char *name;
	/* Hack so we can avoid doing a bunch of allocations */
	struct num_bit bits[BITS_IN(llu)];
};

enum bit_expr_type {
	BO_BIT,
	BO_XOR,
};

struct bit_expr_bit {
	struct num_bit *bit;
};

struct bit_expr;
struct bit_expr_xor {
	struct bit_expr *exprs[2];
};

struct bit_expr {
	enum bit_expr_type type;
	union {
		struct bit_expr_bit bit;
		struct bit_expr_xor xor;
	};
};

#if 0
/* bit_seq_exprs */
/*
	BO_SLICE,
	BO_APPEND,
 */
struct expr_slice {
	struct bit_expr *base;
	unsigned start, end;
};

struct bit_expr_append {
	struct bit_expr *exprs[2];
};
#endif

#if 0
#define BIT_EXPR_BIT(_b) (struct bit_expr){ .type = BO_BIT, .bit = BIT_LIT(_b) }

struct bit_seq {
	darray(struct bit_expr) bits;
};

#define BIT_SEQ_INIT { darray_new() }

static struct bit_seq bit_seq_from_llu(llu v)
{
	struct bit_seq s = BIT_SEQ_INIT;
	uint8_t c = fls(v);
	darray_make_space(s, c);
	uint8_t i;
	for (i = 0; i < c; i++)
		s.bits.item[i] = v & (1 << i);

}
#endif

#if 0
/* XXX: do we want to operate on the bit level and then identify patterns at
 * the end, or operate on larger values and try to massage them at the end to
 * get useful patterns? */

static bool bit_seq_shift_left(struct bit_seq b)
{
}

static void bit_seq_xor(struct bit_seq a, struct bit_seq b)
{

}
#endif

/*
 * Based on crc16 from https://github.com/mattsta/crcspeed.git
 */
static
ull crc_bytes(ull poly, int8_t poly_bits, ull crc, const void *in_data, size_t len, bool augment)
{
	assert((poly_bits % 8) == 0);
	ull high_bit = 1 << (poly_bits - 1);
	const uint8_t *data = in_data;
	for (size_t i = 0; i < len; i++) {
		crc ^= (data[i] << 8);
		for (int j = 0; j < 8; j++) {
			if (crc & high_bit)
				crc = (crc << 1) ^ poly;
			else
				crc <<= 1;
		}
	}

	if (augment) {
		for (size_t i = 0; i < ((size_t)poly_bits / 8); i++) {
			crc ^= (0 << 8);
			for (int j = 0; j < 8; j++) {
				if (crc & high_bit)
					crc = (crc << 1) ^ poly;
				else
					crc <<= 1;
			}
		}
	}
	return crc & bit_mask(poly_bits);
}

/*
 * Based on http://www.zlib.net/crc_v3.txt "SIMPLE" method.
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
 * -----
 *
 * iterates over every single bit, uses an explicit "register" to shift
 * through.
 */
static llu crc_update_simple_(llu msg, int8_t msg_bits,
		llu rem, llu poly, int8_t poly_bits,
		bool lsb_first)
{
	/* Load the register with zero bits. */
	llu reg = rem;
	llu msg_bit_mask = lsb_first
			? 1
			: INTMAX_C(1) << msg_bits;
	llu reg_mask = bit_mask(poly_bits);
	llu reg_bit_mask = 1 << (poly_bits - 1);

	/* While (more message bits) */
	while (msg_bits) {
		bool bit = !!(reg & reg_bit_mask);
		reg <<= 1;

		reg |= !!(msg & msg_bit_mask);
		if (lsb_first)
		    msg_bit_mask <<= 1;
		else
		    msg_bit_mask >>= 1;
		msg_bits--;

		/* if a 1 bit poped out, xor reg with poly */
		if (bit)
			reg ^= poly;
	}

	reg &= reg_mask;
	return reg;
}

#if 0
static llu crc_augment(int8_t bits, llu crc, llu poly)
{
	llu reg_mask = (1 << (bits)) - 1;
	llu reg_bit_mask = 1 << (bits - 1);
	while (--bits) {
		bool bit = !!(crc & reg_bit_mask);
		crc <<= 1;
		/* if a 1 bit poped out, xor reg with poly */
		if (bit)
			crc ^= poly;
	}

	crc &= reg_mask;
	return crc;
}
#endif

/* same as above, but augments */
static llu crc_update_simple(llu msg, int8_t msg_bits,
		llu rem, llu poly, int8_t bits,
		bool lsb_first)
{
	assert((size_t)msg_bits < max_of_u(msg));
	/* Load the register with zero bits. */
	llu reg = rem;

	llu msg_bit_mask = lsb_first
			? 1
			: 1LLU << (msg_bits - 1);
	llu reg_mask = bit_mask(bits);
	llu reg_bit_mask = 1LLU << (bits - 1);

	/* While (more message bits) */
	while (msg_bits) {
		bool bit = !!(reg & reg_bit_mask);
		reg <<= 1;

		reg |= !!(msg & msg_bit_mask);
		if (lsb_first)
		    msg_bit_mask <<= 1;
		else
		    msg_bit_mask >>= 1;
		msg_bits--;

		/* if a 1 bit poped out, xor reg with poly */
		if (bit)
			reg ^= poly;
	}

	while (--bits) {
		bool bit = !!(reg & reg_bit_mask);
		reg <<= 1;
		/* if a 1 bit poped out, xor reg with poly */
		if (bit)
			reg ^= poly;
	}

	reg &= reg_mask;
	return reg;
}

static llu crc_update_simple_bytes(llu crc, llu poly, llu poly_bits,
		uint8_t *msg, size_t msg_bytes,
		bool lsb_first, bool augment)
{
	size_t i;
	for (i = 0; i < msg_bytes; i++)
		crc = crc_update_simple_(msg[i], 8, crc, poly, poly_bits, lsb_first);
	/* augment */
	if (augment)
		crc = crc_update_simple_(0, poly_bits, crc, poly, poly_bits, lsb_first);
	return crc;
}

static llu poly_mult(llu a, llu b)
{
	llu r = 0;
	while (a) {
		if (a & 1)
			r ^= b;
		b <<= 1;
		a >>= 1;
	}

	return r;
}

static llu poly_undiv_(struct llu_pair p, llu m)
{
	return poly_mult(p.a, m) ^ p.b;
}

/* like the below poly_div(), but instead of shifting the denominator down,
 * shift the numerator up */
static struct llu_pair poly_div_shift_numer_(llu numerator, int8_t numer_bits,
		llu denominator, int8_t denom_bits)
{
	assert(numer_bits);
	assert(numer_bits <= 64);
	assert(denom_bits <= 64);

	int8_t denom_shift = numer_bits - denom_bits;
	int8_t denom_shift_orig = denom_shift;
	llu quotient = 0;
	int8_t next_shift_ct = 0;

	if (numer_bits < denom_bits)
		return (struct llu_pair) { quotient, numerator };

	/* align it's first bit with the numerator's first bit */
	denominator <<= denom_shift;

	for (;;) {
		quotient <<= next_shift_ct;
		quotient |= 1;

		numerator ^= denominator;
		next_shift_ct = numer_bits - fls(numerator);

		if (next_shift_ct > denom_shift) {
			quotient  <<= denom_shift;
			numerator >>= denom_shift_orig - denom_shift;
			return (struct llu_pair) { quotient, numerator };
		}

		denom_shift -= next_shift_ct;
		numerator <<= next_shift_ct;
	}
}

static struct llu_pair poly_div_shift_numer(llu numerator, llu denominator)
{
	assert(numerator);
	return poly_div_shift_numer_(numerator, fls_nz(numerator), denominator, fls(denominator));
}

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

static inline
uint8_t lo8(uint16_t val)
{
	return val & 0xff;
}

static inline
uint8_t hi8(uint16_t val)
{
	return val >> 8;
}

/* From avr-libc.
 * They distiguish this from crc-xmodem (same poly) by stating:
 *
 * Although the CCITT polynomial is the same as that used by the Xmodem
 * protocol, they are quite different. The difference is in how the bits are
 * shifted through the alorgithm. Xmodem shifts the MSB of the CRC and the
 * input first, while CCITT shifts the LSB of the CRC and the input first.
 */
static inline
uint16_t crc_ccitt_update(uint16_t crc, uint8_t data)
{
	data ^= lo8(crc);
	data ^= data << 4;

	return ((((uint16_t) data << 8) | hi8(crc)) ^ (uint8_t) (data >> 4)
			^ ((uint16_t) data << 3));
}

static
uint16_t crc_ccitt(uint16_t crc, const void *in_data, size_t len, bool augment)
{
	size_t i;
	for (i = 0; i < len; i++)
		crc = crc_ccitt_update(crc, ((uint8_t *)in_data)[i]);

	if (augment) {
		crc = crc_ccitt_update(crc, 0);
		crc = crc_ccitt_update(crc, 0);
	}

	return crc;
}

#define A(x) (x), sizeof(x)
#define S(x) ((uint8_t *)x), (sizeof(x) - 1)
struct crc_test {
	/* Each bit represents a x^n. Where n = poly_bits, x^n is assumed to
	 * be included */
	ull poly;
	int8_t poly_bits;
	ull init;
	bool augment;
	bool in_lsb_first;
	ull out;
	uint8_t *msg;
	size_t msg_len;
} crc_test[] = {
	/* poly   poly_bits   augment
	 * |	  |   init    |      in_lsb_first
	 * |      |   |       |      |      out     msg
	 * |      |   |       |      |      |       |
	 */

	/* from "crcspeed" */
	{ 0x1021, 16,      0, false, false, 0x31c3, S("123456789") },

	/* from http://srecord.sourceforge.net/crc16-ccitt.html */
	{ 0x1021, 16, 0xffff, true,  false, 0xE5CC, S("123456789") },
	{ 0x1021, 16, 0xffff, true,  false, 0x1D0F, S("") },
	{ 0x1021, 16, 0xffff, true,  false, 0x9479, S("A") },

	/* http://www.boost.org/doc/libs/1_41_0/libs/crc/test/crc_test.cpp */
	/* NOTE: boost identifies this as 'crc-ccitt', but the srecord
	 * crc16-ccitt doc indicates this is wrong due to the lack of
	 * augmentation */
	{ 0x1021, 16, 0xffff, false,  false, 0x29B1, S("123456789") },
};
#define CRC_TEST_FMT "0x%04llx 0x%04llx %d %d 0x%04llx \"%*s\""
#define CRC_TEST_EXP(a) (a).poly, (a).init, (a).augment, (a).in_lsb_first, (a).out, (int)(a).msg_len, (a).msg

#define CRC_TEST(test, calc, name) ok((test)->out == calc, "0x%04llx != 0x%04llx :: " CRC_TEST_FMT " :: " #name, (ull)calc, (test)->out, CRC_TEST_EXP(*(test)))

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
	test_eq_xp(LP(778, 14), poly_div(0x35b0, 0x13));
	test_eq_xp(LP(0, 0x10), poly_div(0x10, 0x10 << 1));
	test_eq_x(14u, crc_update_simple(0x35b, fls(0x35b), 0, 0x13, fls(0x13), false));
	test_eq_xp(LP(778, 14), poly_div_shift_numer(0x35b0, 0x13));
	test_eq_xp(LP(403, 0xf), poly_div_shift_numer(0x35b0, (0x12 << 1) | 1));
	test_eq_x(poly_undiv_(LP(403, 0xf), (0x12 << 1) | 1), 0x35b0);

	size_t i;
	for (i = 0; i < ARRAY_SIZE(crc_test); i++) {
		struct crc_test *t = crc_test + i;
		CRC_TEST(t, crc_update_simple_bytes(t->init, t->poly, t->poly_bits, t->msg, t->msg_len, t->in_lsb_first, t->augment), crc_update_simple_bytes);
	}

	for (i = 0; i < ARRAY_SIZE(crc_test); i++) {
		struct crc_test *t = crc_test + i;
		if (t->poly == 0x1021 && t->poly_bits == 16)
			CRC_TEST(t, crc_ccitt(t->init, t->msg, t->msg_len, t->augment), crc_ccitt);
	}

	for (i = 0; i < ARRAY_SIZE(crc_test); i++) {
		struct crc_test *t = crc_test + i;
		CRC_TEST(t, crc_bytes(t->poly, t->poly_bits, t->init, t->msg, t->msg_len, t->augment), crc_bytes);
	}

	test_done();

	return 0;
}
