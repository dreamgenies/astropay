import { fail, ok } from '@/lib/http';
import { getInvoiceById, recordCheckoutAttempt } from '@/lib/data';
import { buildBuyerPaymentXdr, submitSignedXdr } from '@/lib/stellar';
import { env } from '@/lib/env';

export async function POST(request: Request, { params }: { params: Promise<{ id: string }> }) {
  try {
    const { id } = await params;
    const invoice = await getInvoiceById(id);
    if (!invoice) return fail('Invoice not found', 404);
    const body = await request.json();
    if (body.mode === 'build-xdr') {
      const payer = String(body.payer || '');
      if (!payer) {
        await recordCheckoutAttempt({
          invoiceId: id,
          action: 'build',
          status: 'failed',
          errorDetail: 'Missing payer public key',
        });
        return fail('Missing payer public key');
      }
      let xdr: string;
      try {
        xdr = await buildBuyerPaymentXdr(payer, invoice);
      } catch (error) {
        await recordCheckoutAttempt({
          invoiceId: id,
          action: 'build',
          status: 'failed',
          errorDetail: error instanceof Error ? error.message : 'Failed to build XDR',
          payload: { payer },
        });
        throw error;
      }
      await recordCheckoutAttempt({
        invoiceId: id,
        action: 'build',
        status: 'succeeded',
        payload: { payer },
      });
      return ok({ xdr, networkPassphrase: env.networkPassphrase });
    }
    if (body.mode === 'submit-xdr') {
      const signedXdr = String(body.signedXdr || '');
      if (!signedXdr) {
        await recordCheckoutAttempt({
          invoiceId: id,
          action: 'submit',
          status: 'failed',
          errorDetail: 'Missing signed XDR',
        });
        return fail('Missing signed XDR');
      }
      let submission: Awaited<ReturnType<typeof submitSignedXdr>>;
      try {
        submission = await submitSignedXdr(signedXdr);
      } catch (error) {
        await recordCheckoutAttempt({
          invoiceId: id,
          action: 'submit',
          status: 'failed',
          errorDetail: error instanceof Error ? error.message : 'Failed to submit XDR',
        });
        throw error;
      }
      await recordCheckoutAttempt({
        invoiceId: id,
        action: 'submit',
        status: 'succeeded',
        payload: { hash: submission.hash },
      });
      return ok({ hash: submission.hash });
    }
    return fail('Unsupported mode');
  } catch (error) {
    return fail(error instanceof Error ? error.message : 'Unexpected error', 500);
  }
}
