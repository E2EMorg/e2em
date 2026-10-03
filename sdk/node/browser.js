// Explicit bridge adapter only. No Node socket imports and no browser inference.
export class ExtensionBridge {
  constructor(sendMessage) { this.sendMessage = sendMessage; }
  async call(operation) { return this.sendMessage({type: 'e2em', operation}); }
}
