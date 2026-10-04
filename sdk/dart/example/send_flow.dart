import 'package:e2em_local/e2em.dart';

/// The UI owns the current draft, the confirmation dialog, and actual sending.
Future<bool> assessBeforeSend({
  required Client client,
  required Map<String, dynamic> Function() currentRequest,
  required Future<bool> Function(Assessment) confirmWarning,
  required void Function() sendCurrentDraft,
  CancellationToken? cancellation,
}) async {
  try {
    final report =
        await client.assess(currentRequest(), cancellation: cancellation);
    if (!report.appliesTo(currentRequest())) return false;
    if (report.action == 'warn') {
      if (report.request['policy']?['override'] != 'user_confirm' ||
          !await confirmWarning(report)) return false;
    } else if (report.action != 'allow') {
      return false;
    }
    // Recheck at the send boundary, including after an asynchronous dialog.
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
