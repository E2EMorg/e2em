import 'package:e2em_local/e2em.dart';

/// The UI owns the current draft, advisory warnings, and actual sending.
Future<bool> assessBeforeSend({
  required Client client,
  required Map<String, dynamic> Function() currentRequest,
  required void Function() sendCurrentDraft,
  CancellationToken? cancellation,
}) async {
  try {
    final report =
        await client.assess(currentRequest(), cancellation: cancellation);
    if (!report.appliesTo(currentRequest())) return false;
    if (report.status != 'assessed' ||
        (report.action != 'allow' && report.action != 'warn')) {
      return false;
    }
    // Recheck the current draft at the send boundary.
    if (cancellation?.isCancelled == true ||
        !report.appliesTo(currentRequest())) {
      return false;
    }
    sendCurrentDraft();
    return true;
  } on E2EMError {
    return false;
  }
}
