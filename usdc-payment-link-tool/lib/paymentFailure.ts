export type CheckoutFailureStage = 'wallet' | 'build' | 'submit';

export type CheckoutFailureView = {
  title: string;
  description: string;
  hints: string[];
  /** Raw message for support / debugging; shown in a subdued block */
  technical?: string;
  /** Label for the retry button; omit for non-recoverable failures */
  retryLabel?: string;
};

function uniqueHints(hints: string[]) {
  return [...new Set(hints.filter(Boolean))];
}

/**
 * Maps API / wallet / Horizon strings to buyer-facing copy for checkout.
 * Keeps technical detail separate so the UI never dumps a raw stack-style blob as the only text.
 */
export function resolveCheckoutFailure(rawMessage: string, stage: CheckoutFailureStage): CheckoutFailureView {
  const message = rawMessage.trim() || 'Something went wrong';
  const lower = message.toLowerCase();

  const defaults: Record<CheckoutFailureStage, Omit<CheckoutFailureView, 'technical'>> = {
    wallet: {
      title: 'Wallet blocked this step',
      description: 'Freighter could not finish connecting or signing. No payment was sent.',
      hints: [
        'Confirm the Freighter popup if it is behind this window.',
        'Try Connect Freighter again, then Pay now.',
      ],
    },
    build: {
      title: 'Could not prepare payment',
      description: 'We could not build the Stellar transaction for this invoice. Nothing was submitted to the network.',
      hints: [
        'Refresh the page in case the invoice changed.',
        'If it keeps failing, open the public checkout link from a fresh tab.',
      ],
    },
    submit: {
      title: 'Payment did not go through',
      description: 'The signed transaction was not accepted on the network. Your wallet was not debited by this attempt.',
      hints: [
        'Check you are on the correct Stellar network for this app.',
        'Try Pay now again after a short wait.',
      ],
    },
  };

  const base = defaults[stage];
  const technical = message.length > 180 ? message.slice(0, 180) + '…' : message;

  if (lower.includes('freighter is not connected') || (lower.includes('freighter') && lower.includes('not connected'))) {
    return {
      title: 'Freighter is not available',
      description: 'This browser does not have the Freighter extension connected, so checkout cannot continue.',
      hints: uniqueHints([
        'Install the Freighter browser extension and pin it.',
        'Open Freighter, unlock your wallet, then use Connect Freighter here.',
        ...base.hints,
      ]),
      technical: message,
      retryLabel: 'Connect Freighter',
    };
  }

  if (
    /declin|rejected|denied|cancel|cancell|user rejected|request rejected|not approved/i.test(message) ||
    lower.includes('user denied')
  ) {
    return {
      title: 'You cancelled the wallet prompt',
      description: 'Freighter closed without signing, so no transaction was broadcast.',
      hints: uniqueHints(['Select Pay now again when you are ready to approve in Freighter.', ...base.hints]),
      technical: message,
      retryLabel: 'Try again',
    };
  }

  if (lower.includes('missing payer') || lower.includes('missing payer public key')) {
    return {
      title: 'Wallet address missing',
      description: 'We need a connected Freighter account before we can build your payment.',
      hints: uniqueHints(['Click Connect Freighter and approve access, then Pay now.', ...base.hints]),
      technical: message,
      retryLabel: 'Connect Freighter',
    };
  }

  if (lower.includes('invoice not found')) {
    return {
      title: 'Invoice could not be loaded',
      description: 'The checkout link may be wrong, expired on the server, or the invoice was removed.',
      hints: uniqueHints([
        'Ask the merchant for a new checkout link.',
        'If you saved this tab, open the latest link they sent.',
      ]),
      technical: message,
    };
  }

  if (lower.includes('missing signed xdr') || lower.includes('did not return a signed transaction')) {
    return {
      title: 'No signed transaction from Freighter',
      description: 'Freighter did not return signed transaction data, so nothing could be submitted.',
      hints: uniqueHints([
        'Complete the signing flow in the Freighter window.',
        'If Freighter shows an error about the transaction, fix that issue and try again.',
        ...base.hints,
      ]),
      technical: message,
      retryLabel: 'Try again',
    };
  }

  if (lower.includes('underfunded') || lower.includes('op_underfunded') || lower.includes('insufficient balance')) {
    return {
      title: 'Not enough USDC (or XLM for fees)',
      description: 'Stellar reported insufficient balance for this payment or for the network fee reserve.',
      hints: uniqueHints([
        'Deposit enough USDC for the invoice amount plus a small buffer.',
        'Keep a few XLM in the same account for transaction fees.',
        ...defaults.submit.hints,
      ]),
      technical: message,
    };
  }

  if (lower.includes('op_no_trust') || lower.includes('trustline') || lower.includes('no trust')) {
    return {
      title: 'USDC trustline required',
      description: 'This Stellar account cannot hold the invoice USDC until a trustline is added for that asset.',
      hints: uniqueHints([
        'In Freighter (or your wallet), add a trustline to the USDC issuer this checkout uses.',
        'Ask the merchant which network and USDC issuer they expect if you are unsure.',
        ...defaults.submit.hints,
      ]),
      technical: message,
    };
  }

  if (lower.includes('op_bad_auth') || lower.includes('bad auth')) {
    return {
      title: 'Signing keys did not match the transaction',
      description: 'The network rejected the transaction because the signer did not match what the transaction expects.',
      hints: uniqueHints([
        'Make sure Freighter is using the same account you connected here.',
        ...base.hints,
      ]),
      technical: message,
    };
  }

  if (lower.includes('bad_seq') || lower.includes('tx_bad_seq')) {
    return {
      title: 'Stale sequence number',
      description: 'Another transaction may have updated your account first, so this one was rejected.',
      hints: uniqueHints(['Wait a few seconds and press Pay now again.', ...defaults.submit.hints]),
      technical: message,
      retryLabel: 'Retry payment',
    };
  }

  if (lower.includes('op_line_full') || lower.includes('line full')) {
    return {
      title: 'Destination cannot accept more of this asset',
      description: 'The receiving side hit a Stellar limit for this balance line.',
      hints: uniqueHints([
        'The merchant may need to free capacity on their receiving account.',
        'Try again later or contact the merchant.',
      ]),
      technical: message,
    };
  }

  if (stage === 'build' && (lower.includes('not found') || lower.includes('account not found'))) {
    return {
      title: 'Account not found on the network',
      description: 'Horizon could not load your Stellar account. Brand-new accounts must be funded before they can pay.',
      hints: uniqueHints([
        'Fund this address with a small amount of XLM (and USDC) on the correct network.',
        'Confirm Freighter is pointed at the same network this checkout uses.',
        ...base.hints,
      ]),
      technical: message,
    };
  }

  return {
    title: base.title,
    description: base.description,
    hints: uniqueHints(base.hints),
    technical: message,
  };
}
