import { z } from 'zod';

import { isValidSettlementPublicKey } from '@/lib/stellarPublicKey';

const stellarAccountId = z
  .string()
  .trim()
  .refine((v) => isValidSettlementPublicKey(v), { message: 'Invalid Stellar public key' });

export const registerSchema = z.object({
  email: z.string().email(),
  password: z.string().min(8),
  businessName: z.string().min(2).max(120),
  stellarPublicKey: stellarAccountId,
  settlementPublicKey: stellarAccountId,
});

export const loginSchema = z.object({
  email: z.string().email(),
  password: z.string().min(8),
});

export const invoiceSchema = z.object({
  description: z.string().min(2).max(240),
  amountUsd: z.coerce.number().positive().max(1000000),
});
