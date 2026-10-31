// GENERATED CODE - DO NOT MODIFY BY HAND
// coverage:ignore-file
// ignore_for_file: type=lint, type=warning, deprecated_member_use, deprecated_member_use_from_same_package
// ignore_for_file: unused_element, deprecated_member_use, deprecated_member_use_from_same_package, use_function_type_syntax_for_parameters, unnecessary_const, avoid_init_to_null, invalid_override_different_default_values_named, prefer_expression_function_bodies, annotate_overrides, invalid_annotation_target, unnecessary_question_mark

part of 'nudge.dart';

// **************************************************************************
// FreezedGenerator
// **************************************************************************

// GENERATED CODE - DO NOT MODIFY BY HAND
// dart format off
T _$identity<T>(T value) => value;
/// @nodoc
mixin _$AppEvent {





@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'AppEvent()';
}


}

/// @nodoc
class $AppEventCopyWith<$Res>  {
$AppEventCopyWith(AppEvent _, $Res Function(AppEvent) __);
}


/// Adds pattern-matching-related methods to [AppEvent].
extension AppEventPatterns on AppEvent {
/// A variant of `map` that fallback to returning `orElse`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( AppEvent_DevicesChanged value)?  devicesChanged,TResult Function( AppEvent_Paired value)?  paired,TResult Function( AppEvent_PairingClosed value)?  pairingClosed,TResult Function( AppEvent_TransferStarted value)?  transferStarted,TResult Function( AppEvent_TransferProgress value)?  transferProgress,TResult Function( AppEvent_TransferDone value)?  transferDone,TResult Function( AppEvent_TransferFailed value)?  transferFailed,TResult Function( AppEvent_WebShareStopped value)?  webShareStopped,TResult Function( AppEvent_OfferReceived value)?  offerReceived,TResult Function( AppEvent_OfferClosed value)?  offerClosed,TResult Function( AppEvent_TransferAccepted value)?  transferAccepted,TResult Function( AppEvent_TransferPending value)?  transferPending,TResult Function( AppEvent_TransferDelivered value)?  transferDelivered,TResult Function( AppEvent_TransferRoute value)?  transferRoute,TResult Function( AppEvent_ClipboardReceived value)?  clipboardReceived,TResult Function( AppEvent_ClipboardImageReceived value)?  clipboardImageReceived,TResult Function( AppEvent_HotspotWanted value)?  hotspotWanted,TResult Function( AppEvent_HotspotDone value)?  hotspotDone,TResult Function( AppEvent_JoinWanted value)?  joinWanted,TResult Function( AppEvent_LeaveWanted value)?  leaveWanted,TResult Function( AppEvent_PairAsked value)?  pairAsked,TResult Function( AppEvent_MirrorAsked value)?  mirrorAsked,TResult Function( AppEvent_MirrorStarted value)?  mirrorStarted,TResult Function( AppEvent_MirrorWaiting value)?  mirrorWaiting,TResult Function( AppEvent_MirrorEnded value)?  mirrorEnded,required TResult orElse(),}){
final _that = this;
switch (_that) {
case AppEvent_DevicesChanged() when devicesChanged != null:
return devicesChanged(_that);case AppEvent_Paired() when paired != null:
return paired(_that);case AppEvent_PairingClosed() when pairingClosed != null:
return pairingClosed(_that);case AppEvent_TransferStarted() when transferStarted != null:
return transferStarted(_that);case AppEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that);case AppEvent_TransferDone() when transferDone != null:
return transferDone(_that);case AppEvent_TransferFailed() when transferFailed != null:
return transferFailed(_that);case AppEvent_WebShareStopped() when webShareStopped != null:
return webShareStopped(_that);case AppEvent_OfferReceived() when offerReceived != null:
return offerReceived(_that);case AppEvent_OfferClosed() when offerClosed != null:
return offerClosed(_that);case AppEvent_TransferAccepted() when transferAccepted != null:
return transferAccepted(_that);case AppEvent_TransferPending() when transferPending != null:
return transferPending(_that);case AppEvent_TransferDelivered() when transferDelivered != null:
return transferDelivered(_that);case AppEvent_TransferRoute() when transferRoute != null:
return transferRoute(_that);case AppEvent_ClipboardReceived() when clipboardReceived != null:
return clipboardReceived(_that);case AppEvent_ClipboardImageReceived() when clipboardImageReceived != null:
return clipboardImageReceived(_that);case AppEvent_HotspotWanted() when hotspotWanted != null:
return hotspotWanted(_that);case AppEvent_HotspotDone() when hotspotDone != null:
return hotspotDone(_that);case AppEvent_JoinWanted() when joinWanted != null:
return joinWanted(_that);case AppEvent_LeaveWanted() when leaveWanted != null:
return leaveWanted(_that);case AppEvent_PairAsked() when pairAsked != null:
return pairAsked(_that);case AppEvent_MirrorAsked() when mirrorAsked != null:
return mirrorAsked(_that);case AppEvent_MirrorStarted() when mirrorStarted != null:
return mirrorStarted(_that);case AppEvent_MirrorWaiting() when mirrorWaiting != null:
return mirrorWaiting(_that);case AppEvent_MirrorEnded() when mirrorEnded != null:
return mirrorEnded(_that);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// Callbacks receives the raw object, upcasted.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case final Subclass2 value:
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( AppEvent_DevicesChanged value)  devicesChanged,required TResult Function( AppEvent_Paired value)  paired,required TResult Function( AppEvent_PairingClosed value)  pairingClosed,required TResult Function( AppEvent_TransferStarted value)  transferStarted,required TResult Function( AppEvent_TransferProgress value)  transferProgress,required TResult Function( AppEvent_TransferDone value)  transferDone,required TResult Function( AppEvent_TransferFailed value)  transferFailed,required TResult Function( AppEvent_WebShareStopped value)  webShareStopped,required TResult Function( AppEvent_OfferReceived value)  offerReceived,required TResult Function( AppEvent_OfferClosed value)  offerClosed,required TResult Function( AppEvent_TransferAccepted value)  transferAccepted,required TResult Function( AppEvent_TransferPending value)  transferPending,required TResult Function( AppEvent_TransferDelivered value)  transferDelivered,required TResult Function( AppEvent_TransferRoute value)  transferRoute,required TResult Function( AppEvent_ClipboardReceived value)  clipboardReceived,required TResult Function( AppEvent_ClipboardImageReceived value)  clipboardImageReceived,required TResult Function( AppEvent_HotspotWanted value)  hotspotWanted,required TResult Function( AppEvent_HotspotDone value)  hotspotDone,required TResult Function( AppEvent_JoinWanted value)  joinWanted,required TResult Function( AppEvent_LeaveWanted value)  leaveWanted,required TResult Function( AppEvent_PairAsked value)  pairAsked,required TResult Function( AppEvent_MirrorAsked value)  mirrorAsked,required TResult Function( AppEvent_MirrorStarted value)  mirrorStarted,required TResult Function( AppEvent_MirrorWaiting value)  mirrorWaiting,required TResult Function( AppEvent_MirrorEnded value)  mirrorEnded,}){
final _that = this;
switch (_that) {
case AppEvent_DevicesChanged():
return devicesChanged(_that);case AppEvent_Paired():
return paired(_that);case AppEvent_PairingClosed():
return pairingClosed(_that);case AppEvent_TransferStarted():
return transferStarted(_that);case AppEvent_TransferProgress():
return transferProgress(_that);case AppEvent_TransferDone():
return transferDone(_that);case AppEvent_TransferFailed():
return transferFailed(_that);case AppEvent_WebShareStopped():
return webShareStopped(_that);case AppEvent_OfferReceived():
return offerReceived(_that);case AppEvent_OfferClosed():
return offerClosed(_that);case AppEvent_TransferAccepted():
return transferAccepted(_that);case AppEvent_TransferPending():
return transferPending(_that);case AppEvent_TransferDelivered():
return transferDelivered(_that);case AppEvent_TransferRoute():
return transferRoute(_that);case AppEvent_ClipboardReceived():
return clipboardReceived(_that);case AppEvent_ClipboardImageReceived():
return clipboardImageReceived(_that);case AppEvent_HotspotWanted():
return hotspotWanted(_that);case AppEvent_HotspotDone():
return hotspotDone(_that);case AppEvent_JoinWanted():
return joinWanted(_that);case AppEvent_LeaveWanted():
return leaveWanted(_that);case AppEvent_PairAsked():
return pairAsked(_that);case AppEvent_MirrorAsked():
return mirrorAsked(_that);case AppEvent_MirrorStarted():
return mirrorStarted(_that);case AppEvent_MirrorWaiting():
return mirrorWaiting(_that);case AppEvent_MirrorEnded():
return mirrorEnded(_that);}
}
/// A variant of `map` that fallback to returning `null`.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case final Subclass value:
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( AppEvent_DevicesChanged value)?  devicesChanged,TResult? Function( AppEvent_Paired value)?  paired,TResult? Function( AppEvent_PairingClosed value)?  pairingClosed,TResult? Function( AppEvent_TransferStarted value)?  transferStarted,TResult? Function( AppEvent_TransferProgress value)?  transferProgress,TResult? Function( AppEvent_TransferDone value)?  transferDone,TResult? Function( AppEvent_TransferFailed value)?  transferFailed,TResult? Function( AppEvent_WebShareStopped value)?  webShareStopped,TResult? Function( AppEvent_OfferReceived value)?  offerReceived,TResult? Function( AppEvent_OfferClosed value)?  offerClosed,TResult? Function( AppEvent_TransferAccepted value)?  transferAccepted,TResult? Function( AppEvent_TransferPending value)?  transferPending,TResult? Function( AppEvent_TransferDelivered value)?  transferDelivered,TResult? Function( AppEvent_TransferRoute value)?  transferRoute,TResult? Function( AppEvent_ClipboardReceived value)?  clipboardReceived,TResult? Function( AppEvent_ClipboardImageReceived value)?  clipboardImageReceived,TResult? Function( AppEvent_HotspotWanted value)?  hotspotWanted,TResult? Function( AppEvent_HotspotDone value)?  hotspotDone,TResult? Function( AppEvent_JoinWanted value)?  joinWanted,TResult? Function( AppEvent_LeaveWanted value)?  leaveWanted,TResult? Function( AppEvent_PairAsked value)?  pairAsked,TResult? Function( AppEvent_MirrorAsked value)?  mirrorAsked,TResult? Function( AppEvent_MirrorStarted value)?  mirrorStarted,TResult? Function( AppEvent_MirrorWaiting value)?  mirrorWaiting,TResult? Function( AppEvent_MirrorEnded value)?  mirrorEnded,}){
final _that = this;
switch (_that) {
case AppEvent_DevicesChanged() when devicesChanged != null:
return devicesChanged(_that);case AppEvent_Paired() when paired != null:
return paired(_that);case AppEvent_PairingClosed() when pairingClosed != null:
return pairingClosed(_that);case AppEvent_TransferStarted() when transferStarted != null:
return transferStarted(_that);case AppEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that);case AppEvent_TransferDone() when transferDone != null:
return transferDone(_that);case AppEvent_TransferFailed() when transferFailed != null:
return transferFailed(_that);case AppEvent_WebShareStopped() when webShareStopped != null:
return webShareStopped(_that);case AppEvent_OfferReceived() when offerReceived != null:
return offerReceived(_that);case AppEvent_OfferClosed() when offerClosed != null:
return offerClosed(_that);case AppEvent_TransferAccepted() when transferAccepted != null:
return transferAccepted(_that);case AppEvent_TransferPending() when transferPending != null:
return transferPending(_that);case AppEvent_TransferDelivered() when transferDelivered != null:
return transferDelivered(_that);case AppEvent_TransferRoute() when transferRoute != null:
return transferRoute(_that);case AppEvent_ClipboardReceived() when clipboardReceived != null:
return clipboardReceived(_that);case AppEvent_ClipboardImageReceived() when clipboardImageReceived != null:
return clipboardImageReceived(_that);case AppEvent_HotspotWanted() when hotspotWanted != null:
return hotspotWanted(_that);case AppEvent_HotspotDone() when hotspotDone != null:
return hotspotDone(_that);case AppEvent_JoinWanted() when joinWanted != null:
return joinWanted(_that);case AppEvent_LeaveWanted() when leaveWanted != null:
return leaveWanted(_that);case AppEvent_PairAsked() when pairAsked != null:
return pairAsked(_that);case AppEvent_MirrorAsked() when mirrorAsked != null:
return mirrorAsked(_that);case AppEvent_MirrorStarted() when mirrorStarted != null:
return mirrorStarted(_that);case AppEvent_MirrorWaiting() when mirrorWaiting != null:
return mirrorWaiting(_that);case AppEvent_MirrorEnded() when mirrorEnded != null:
return mirrorEnded(_that);case _:
  return null;

}
}
/// A variant of `when` that fallback to an `orElse` callback.
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return orElse();
/// }
/// ```

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  devicesChanged,TResult Function( DeviceInfo device)?  paired,TResult Function()?  pairingClosed,TResult Function( PlatformInt64 id,  String deviceId,  bool incoming,  String fileName,  PlatformInt64 size)?  transferStarted,TResult Function( PlatformInt64 id,  PlatformInt64 bytes,  PlatformInt64 size,  double bytesPerSec)?  transferProgress,TResult Function( PlatformInt64 id,  String path,  PlatformInt64 bytes,  double seconds,  PlatformInt64 resumedFrom)?  transferDone,TResult Function( PlatformInt64 id,  String error)?  transferFailed,TResult Function()?  webShareStopped,TResult Function( PlatformInt64 id,  String deviceId,  String fileName,  PlatformInt64 count,  PlatformInt64 size)?  offerReceived,TResult Function( PlatformInt64 id)?  offerClosed,TResult Function( PlatformInt64 id)?  transferAccepted,TResult Function( PlatformInt64 id)?  transferPending,TResult Function( PlatformInt64 id)?  transferDelivered,TResult Function( PlatformInt64 id,  String route)?  transferRoute,TResult Function( String deviceId,  String text)?  clipboardReceived,TResult Function( String deviceId,  String path,  String mime)?  clipboardImageReceived,TResult Function( PlatformInt64 id,  bool directOnly)?  hotspotWanted,TResult Function()?  hotspotDone,TResult Function( PlatformInt64 id,  String ssid,  String passphrase,  bool wpa3)?  joinWanted,TResult Function()?  leaveWanted,TResult Function( PlatformInt64 id,  String deviceId,  String deviceName,  String code)?  pairAsked,TResult Function( PlatformInt64 id,  String deviceId)?  mirrorAsked,TResult Function( PlatformInt64 id,  String deviceId,  bool sharing)?  mirrorStarted,TResult Function( String deviceId)?  mirrorWaiting,TResult Function( PlatformInt64 id)?  mirrorEnded,required TResult orElse(),}) {final _that = this;
switch (_that) {
case AppEvent_DevicesChanged() when devicesChanged != null:
return devicesChanged();case AppEvent_Paired() when paired != null:
return paired(_that.device);case AppEvent_PairingClosed() when pairingClosed != null:
return pairingClosed();case AppEvent_TransferStarted() when transferStarted != null:
return transferStarted(_that.id,_that.deviceId,_that.incoming,_that.fileName,_that.size);case AppEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that.id,_that.bytes,_that.size,_that.bytesPerSec);case AppEvent_TransferDone() when transferDone != null:
return transferDone(_that.id,_that.path,_that.bytes,_that.seconds,_that.resumedFrom);case AppEvent_TransferFailed() when transferFailed != null:
return transferFailed(_that.id,_that.error);case AppEvent_WebShareStopped() when webShareStopped != null:
return webShareStopped();case AppEvent_OfferReceived() when offerReceived != null:
return offerReceived(_that.id,_that.deviceId,_that.fileName,_that.count,_that.size);case AppEvent_OfferClosed() when offerClosed != null:
return offerClosed(_that.id);case AppEvent_TransferAccepted() when transferAccepted != null:
return transferAccepted(_that.id);case AppEvent_TransferPending() when transferPending != null:
return transferPending(_that.id);case AppEvent_TransferDelivered() when transferDelivered != null:
return transferDelivered(_that.id);case AppEvent_TransferRoute() when transferRoute != null:
return transferRoute(_that.id,_that.route);case AppEvent_ClipboardReceived() when clipboardReceived != null:
return clipboardReceived(_that.deviceId,_that.text);case AppEvent_ClipboardImageReceived() when clipboardImageReceived != null:
return clipboardImageReceived(_that.deviceId,_that.path,_that.mime);case AppEvent_HotspotWanted() when hotspotWanted != null:
return hotspotWanted(_that.id,_that.directOnly);case AppEvent_HotspotDone() when hotspotDone != null:
return hotspotDone();case AppEvent_JoinWanted() when joinWanted != null:
return joinWanted(_that.id,_that.ssid,_that.passphrase,_that.wpa3);case AppEvent_LeaveWanted() when leaveWanted != null:
return leaveWanted();case AppEvent_PairAsked() when pairAsked != null:
return pairAsked(_that.id,_that.deviceId,_that.deviceName,_that.code);case AppEvent_MirrorAsked() when mirrorAsked != null:
return mirrorAsked(_that.id,_that.deviceId);case AppEvent_MirrorStarted() when mirrorStarted != null:
return mirrorStarted(_that.id,_that.deviceId,_that.sharing);case AppEvent_MirrorWaiting() when mirrorWaiting != null:
return mirrorWaiting(_that.deviceId);case AppEvent_MirrorEnded() when mirrorEnded != null:
return mirrorEnded(_that.id);case _:
  return orElse();

}
}
/// A `switch`-like method, using callbacks.
///
/// As opposed to `map`, this offers destructuring.
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case Subclass2(:final field2):
///     return ...;
/// }
/// ```

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  devicesChanged,required TResult Function( DeviceInfo device)  paired,required TResult Function()  pairingClosed,required TResult Function( PlatformInt64 id,  String deviceId,  bool incoming,  String fileName,  PlatformInt64 size)  transferStarted,required TResult Function( PlatformInt64 id,  PlatformInt64 bytes,  PlatformInt64 size,  double bytesPerSec)  transferProgress,required TResult Function( PlatformInt64 id,  String path,  PlatformInt64 bytes,  double seconds,  PlatformInt64 resumedFrom)  transferDone,required TResult Function( PlatformInt64 id,  String error)  transferFailed,required TResult Function()  webShareStopped,required TResult Function( PlatformInt64 id,  String deviceId,  String fileName,  PlatformInt64 count,  PlatformInt64 size)  offerReceived,required TResult Function( PlatformInt64 id)  offerClosed,required TResult Function( PlatformInt64 id)  transferAccepted,required TResult Function( PlatformInt64 id)  transferPending,required TResult Function( PlatformInt64 id)  transferDelivered,required TResult Function( PlatformInt64 id,  String route)  transferRoute,required TResult Function( String deviceId,  String text)  clipboardReceived,required TResult Function( String deviceId,  String path,  String mime)  clipboardImageReceived,required TResult Function( PlatformInt64 id,  bool directOnly)  hotspotWanted,required TResult Function()  hotspotDone,required TResult Function( PlatformInt64 id,  String ssid,  String passphrase,  bool wpa3)  joinWanted,required TResult Function()  leaveWanted,required TResult Function( PlatformInt64 id,  String deviceId,  String deviceName,  String code)  pairAsked,required TResult Function( PlatformInt64 id,  String deviceId)  mirrorAsked,required TResult Function( PlatformInt64 id,  String deviceId,  bool sharing)  mirrorStarted,required TResult Function( String deviceId)  mirrorWaiting,required TResult Function( PlatformInt64 id)  mirrorEnded,}) {final _that = this;
switch (_that) {
case AppEvent_DevicesChanged():
return devicesChanged();case AppEvent_Paired():
return paired(_that.device);case AppEvent_PairingClosed():
return pairingClosed();case AppEvent_TransferStarted():
return transferStarted(_that.id,_that.deviceId,_that.incoming,_that.fileName,_that.size);case AppEvent_TransferProgress():
return transferProgress(_that.id,_that.bytes,_that.size,_that.bytesPerSec);case AppEvent_TransferDone():
return transferDone(_that.id,_that.path,_that.bytes,_that.seconds,_that.resumedFrom);case AppEvent_TransferFailed():
return transferFailed(_that.id,_that.error);case AppEvent_WebShareStopped():
return webShareStopped();case AppEvent_OfferReceived():
return offerReceived(_that.id,_that.deviceId,_that.fileName,_that.count,_that.size);case AppEvent_OfferClosed():
return offerClosed(_that.id);case AppEvent_TransferAccepted():
return transferAccepted(_that.id);case AppEvent_TransferPending():
return transferPending(_that.id);case AppEvent_TransferDelivered():
return transferDelivered(_that.id);case AppEvent_TransferRoute():
return transferRoute(_that.id,_that.route);case AppEvent_ClipboardReceived():
return clipboardReceived(_that.deviceId,_that.text);case AppEvent_ClipboardImageReceived():
return clipboardImageReceived(_that.deviceId,_that.path,_that.mime);case AppEvent_HotspotWanted():
return hotspotWanted(_that.id,_that.directOnly);case AppEvent_HotspotDone():
return hotspotDone();case AppEvent_JoinWanted():
return joinWanted(_that.id,_that.ssid,_that.passphrase,_that.wpa3);case AppEvent_LeaveWanted():
return leaveWanted();case AppEvent_PairAsked():
return pairAsked(_that.id,_that.deviceId,_that.deviceName,_that.code);case AppEvent_MirrorAsked():
return mirrorAsked(_that.id,_that.deviceId);case AppEvent_MirrorStarted():
return mirrorStarted(_that.id,_that.deviceId,_that.sharing);case AppEvent_MirrorWaiting():
return mirrorWaiting(_that.deviceId);case AppEvent_MirrorEnded():
return mirrorEnded(_that.id);}
}
/// A variant of `when` that fallback to returning `null`
///
/// It is equivalent to doing:
/// ```dart
/// switch (sealedClass) {
///   case Subclass(:final field):
///     return ...;
///   case _:
///     return null;
/// }
/// ```

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  devicesChanged,TResult? Function( DeviceInfo device)?  paired,TResult? Function()?  pairingClosed,TResult? Function( PlatformInt64 id,  String deviceId,  bool incoming,  String fileName,  PlatformInt64 size)?  transferStarted,TResult? Function( PlatformInt64 id,  PlatformInt64 bytes,  PlatformInt64 size,  double bytesPerSec)?  transferProgress,TResult? Function( PlatformInt64 id,  String path,  PlatformInt64 bytes,  double seconds,  PlatformInt64 resumedFrom)?  transferDone,TResult? Function( PlatformInt64 id,  String error)?  transferFailed,TResult? Function()?  webShareStopped,TResult? Function( PlatformInt64 id,  String deviceId,  String fileName,  PlatformInt64 count,  PlatformInt64 size)?  offerReceived,TResult? Function( PlatformInt64 id)?  offerClosed,TResult? Function( PlatformInt64 id)?  transferAccepted,TResult? Function( PlatformInt64 id)?  transferPending,TResult? Function( PlatformInt64 id)?  transferDelivered,TResult? Function( PlatformInt64 id,  String route)?  transferRoute,TResult? Function( String deviceId,  String text)?  clipboardReceived,TResult? Function( String deviceId,  String path,  String mime)?  clipboardImageReceived,TResult? Function( PlatformInt64 id,  bool directOnly)?  hotspotWanted,TResult? Function()?  hotspotDone,TResult? Function( PlatformInt64 id,  String ssid,  String passphrase,  bool wpa3)?  joinWanted,TResult? Function()?  leaveWanted,TResult? Function( PlatformInt64 id,  String deviceId,  String deviceName,  String code)?  pairAsked,TResult? Function( PlatformInt64 id,  String deviceId)?  mirrorAsked,TResult? Function( PlatformInt64 id,  String deviceId,  bool sharing)?  mirrorStarted,TResult? Function( String deviceId)?  mirrorWaiting,TResult? Function( PlatformInt64 id)?  mirrorEnded,}) {final _that = this;
switch (_that) {
case AppEvent_DevicesChanged() when devicesChanged != null:
return devicesChanged();case AppEvent_Paired() when paired != null:
return paired(_that.device);case AppEvent_PairingClosed() when pairingClosed != null:
return pairingClosed();case AppEvent_TransferStarted() when transferStarted != null:
return transferStarted(_that.id,_that.deviceId,_that.incoming,_that.fileName,_that.size);case AppEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that.id,_that.bytes,_that.size,_that.bytesPerSec);case AppEvent_TransferDone() when transferDone != null:
return transferDone(_that.id,_that.path,_that.bytes,_that.seconds,_that.resumedFrom);case AppEvent_TransferFailed() when transferFailed != null:
return transferFailed(_that.id,_that.error);case AppEvent_WebShareStopped() when webShareStopped != null:
return webShareStopped();case AppEvent_OfferReceived() when offerReceived != null:
return offerReceived(_that.id,_that.deviceId,_that.fileName,_that.count,_that.size);case AppEvent_OfferClosed() when offerClosed != null:
return offerClosed(_that.id);case AppEvent_TransferAccepted() when transferAccepted != null:
return transferAccepted(_that.id);case AppEvent_TransferPending() when transferPending != null:
return transferPending(_that.id);case AppEvent_TransferDelivered() when transferDelivered != null:
return transferDelivered(_that.id);case AppEvent_TransferRoute() when transferRoute != null:
return transferRoute(_that.id,_that.route);case AppEvent_ClipboardReceived() when clipboardReceived != null:
return clipboardReceived(_that.deviceId,_that.text);case AppEvent_ClipboardImageReceived() when clipboardImageReceived != null:
return clipboardImageReceived(_that.deviceId,_that.path,_that.mime);case AppEvent_HotspotWanted() when hotspotWanted != null:
return hotspotWanted(_that.id,_that.directOnly);case AppEvent_HotspotDone() when hotspotDone != null:
return hotspotDone();case AppEvent_JoinWanted() when joinWanted != null:
return joinWanted(_that.id,_that.ssid,_that.passphrase,_that.wpa3);case AppEvent_LeaveWanted() when leaveWanted != null:
return leaveWanted();case AppEvent_PairAsked() when pairAsked != null:
return pairAsked(_that.id,_that.deviceId,_that.deviceName,_that.code);case AppEvent_MirrorAsked() when mirrorAsked != null:
return mirrorAsked(_that.id,_that.deviceId);case AppEvent_MirrorStarted() when mirrorStarted != null:
return mirrorStarted(_that.id,_that.deviceId,_that.sharing);case AppEvent_MirrorWaiting() when mirrorWaiting != null:
return mirrorWaiting(_that.deviceId);case AppEvent_MirrorEnded() when mirrorEnded != null:
return mirrorEnded(_that.id);case _:
  return null;

}
}

}

/// @nodoc


class AppEvent_DevicesChanged extends AppEvent {
  const AppEvent_DevicesChanged(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_DevicesChanged);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'AppEvent.devicesChanged()';
}


}




/// @nodoc


class AppEvent_Paired extends AppEvent {
  const AppEvent_Paired({required this.device}): super._();
  

 final  DeviceInfo device;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_PairedCopyWith<AppEvent_Paired> get copyWith => _$AppEvent_PairedCopyWithImpl<AppEvent_Paired>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_Paired&&(identical(other.device, device) || other.device == device));
}


@override
int get hashCode {
    return Object.hash(runtimeType,device);
}

@override
String toString() {
    return 'AppEvent.paired(device: $device)';
}


}

/// @nodoc
abstract mixin class $AppEvent_PairedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_PairedCopyWith(AppEvent_Paired value, $Res Function(AppEvent_Paired) _then) = _$AppEvent_PairedCopyWithImpl;
@useResult
$Res call({
 DeviceInfo device
});




}
/// @nodoc
class _$AppEvent_PairedCopyWithImpl<$Res>
    implements $AppEvent_PairedCopyWith<$Res> {
  _$AppEvent_PairedCopyWithImpl(this._self, this._then);

  final AppEvent_Paired _self;
  final $Res Function(AppEvent_Paired) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? device = null,}) {
  return _then(AppEvent_Paired(
device: null == device ? _self.device : device // ignore: cast_nullable_to_non_nullable
as DeviceInfo,
  ));
}


}

/// @nodoc


class AppEvent_PairingClosed extends AppEvent {
  const AppEvent_PairingClosed(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_PairingClosed);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'AppEvent.pairingClosed()';
}


}




/// @nodoc


class AppEvent_TransferStarted extends AppEvent {
  const AppEvent_TransferStarted({required this.id, required this.deviceId, required this.incoming, required this.fileName, required this.size}): super._();
  

 final  PlatformInt64 id;
 final  String deviceId;
 final  bool incoming;
 final  String fileName;
 final  PlatformInt64 size;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_TransferStartedCopyWith<AppEvent_TransferStarted> get copyWith => _$AppEvent_TransferStartedCopyWithImpl<AppEvent_TransferStarted>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_TransferStarted&&(identical(other.id, id) || other.id == id)&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.incoming, incoming) || other.incoming == incoming)&&(identical(other.fileName, fileName) || other.fileName == fileName)&&(identical(other.size, size) || other.size == size));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,deviceId,incoming,fileName,size);
}

@override
String toString() {
    return 'AppEvent.transferStarted(id: $id, deviceId: $deviceId, incoming: $incoming, fileName: $fileName, size: $size)';
}


}

/// @nodoc
abstract mixin class $AppEvent_TransferStartedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_TransferStartedCopyWith(AppEvent_TransferStarted value, $Res Function(AppEvent_TransferStarted) _then) = _$AppEvent_TransferStartedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String deviceId, bool incoming, String fileName, PlatformInt64 size
});




}
/// @nodoc
class _$AppEvent_TransferStartedCopyWithImpl<$Res>
    implements $AppEvent_TransferStartedCopyWith<$Res> {
  _$AppEvent_TransferStartedCopyWithImpl(this._self, this._then);

  final AppEvent_TransferStarted _self;
  final $Res Function(AppEvent_TransferStarted) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? deviceId = null,Object? incoming = null,Object? fileName = null,Object? size = null,}) {
  return _then(AppEvent_TransferStarted(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,incoming: null == incoming ? _self.incoming : incoming // ignore: cast_nullable_to_non_nullable
as bool,fileName: null == fileName ? _self.fileName : fileName // ignore: cast_nullable_to_non_nullable
as String,size: null == size ? _self.size : size // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class AppEvent_TransferProgress extends AppEvent {
  const AppEvent_TransferProgress({required this.id, required this.bytes, required this.size, required this.bytesPerSec}): super._();
  

 final  PlatformInt64 id;
 final  PlatformInt64 bytes;
 final  PlatformInt64 size;
 final  double bytesPerSec;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_TransferProgressCopyWith<AppEvent_TransferProgress> get copyWith => _$AppEvent_TransferProgressCopyWithImpl<AppEvent_TransferProgress>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_TransferProgress&&(identical(other.id, id) || other.id == id)&&(identical(other.bytes, bytes) || other.bytes == bytes)&&(identical(other.size, size) || other.size == size)&&(identical(other.bytesPerSec, bytesPerSec) || other.bytesPerSec == bytesPerSec));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,bytes,size,bytesPerSec);
}

@override
String toString() {
    return 'AppEvent.transferProgress(id: $id, bytes: $bytes, size: $size, bytesPerSec: $bytesPerSec)';
}


}

/// @nodoc
abstract mixin class $AppEvent_TransferProgressCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_TransferProgressCopyWith(AppEvent_TransferProgress value, $Res Function(AppEvent_TransferProgress) _then) = _$AppEvent_TransferProgressCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, PlatformInt64 bytes, PlatformInt64 size, double bytesPerSec
});




}
/// @nodoc
class _$AppEvent_TransferProgressCopyWithImpl<$Res>
    implements $AppEvent_TransferProgressCopyWith<$Res> {
  _$AppEvent_TransferProgressCopyWithImpl(this._self, this._then);

  final AppEvent_TransferProgress _self;
  final $Res Function(AppEvent_TransferProgress) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? bytes = null,Object? size = null,Object? bytesPerSec = null,}) {
  return _then(AppEvent_TransferProgress(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,bytes: null == bytes ? _self.bytes : bytes // ignore: cast_nullable_to_non_nullable
as PlatformInt64,size: null == size ? _self.size : size // ignore: cast_nullable_to_non_nullable
as PlatformInt64,bytesPerSec: null == bytesPerSec ? _self.bytesPerSec : bytesPerSec // ignore: cast_nullable_to_non_nullable
as double,
  ));
}


}

/// @nodoc


class AppEvent_TransferDone extends AppEvent {
  const AppEvent_TransferDone({required this.id, required this.path, required this.bytes, required this.seconds, required this.resumedFrom}): super._();
  

 final  PlatformInt64 id;
/// Saved file (incoming) or source file (outgoing).
 final  String path;
 final  PlatformInt64 bytes;
 final  double seconds;
 final  PlatformInt64 resumedFrom;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_TransferDoneCopyWith<AppEvent_TransferDone> get copyWith => _$AppEvent_TransferDoneCopyWithImpl<AppEvent_TransferDone>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_TransferDone&&(identical(other.id, id) || other.id == id)&&(identical(other.path, path) || other.path == path)&&(identical(other.bytes, bytes) || other.bytes == bytes)&&(identical(other.seconds, seconds) || other.seconds == seconds)&&(identical(other.resumedFrom, resumedFrom) || other.resumedFrom == resumedFrom));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,path,bytes,seconds,resumedFrom);
}

@override
String toString() {
    return 'AppEvent.transferDone(id: $id, path: $path, bytes: $bytes, seconds: $seconds, resumedFrom: $resumedFrom)';
}


}

/// @nodoc
abstract mixin class $AppEvent_TransferDoneCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_TransferDoneCopyWith(AppEvent_TransferDone value, $Res Function(AppEvent_TransferDone) _then) = _$AppEvent_TransferDoneCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String path, PlatformInt64 bytes, double seconds, PlatformInt64 resumedFrom
});




}
/// @nodoc
class _$AppEvent_TransferDoneCopyWithImpl<$Res>
    implements $AppEvent_TransferDoneCopyWith<$Res> {
  _$AppEvent_TransferDoneCopyWithImpl(this._self, this._then);

  final AppEvent_TransferDone _self;
  final $Res Function(AppEvent_TransferDone) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? path = null,Object? bytes = null,Object? seconds = null,Object? resumedFrom = null,}) {
  return _then(AppEvent_TransferDone(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,path: null == path ? _self.path : path // ignore: cast_nullable_to_non_nullable
as String,bytes: null == bytes ? _self.bytes : bytes // ignore: cast_nullable_to_non_nullable
as PlatformInt64,seconds: null == seconds ? _self.seconds : seconds // ignore: cast_nullable_to_non_nullable
as double,resumedFrom: null == resumedFrom ? _self.resumedFrom : resumedFrom // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class AppEvent_TransferFailed extends AppEvent {
  const AppEvent_TransferFailed({required this.id, required this.error}): super._();
  

 final  PlatformInt64 id;
 final  String error;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_TransferFailedCopyWith<AppEvent_TransferFailed> get copyWith => _$AppEvent_TransferFailedCopyWithImpl<AppEvent_TransferFailed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_TransferFailed&&(identical(other.id, id) || other.id == id)&&(identical(other.error, error) || other.error == error));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,error);
}

@override
String toString() {
    return 'AppEvent.transferFailed(id: $id, error: $error)';
}


}

/// @nodoc
abstract mixin class $AppEvent_TransferFailedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_TransferFailedCopyWith(AppEvent_TransferFailed value, $Res Function(AppEvent_TransferFailed) _then) = _$AppEvent_TransferFailedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String error
});




}
/// @nodoc
class _$AppEvent_TransferFailedCopyWithImpl<$Res>
    implements $AppEvent_TransferFailedCopyWith<$Res> {
  _$AppEvent_TransferFailedCopyWithImpl(this._self, this._then);

  final AppEvent_TransferFailed _self;
  final $Res Function(AppEvent_TransferFailed) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? error = null,}) {
  return _then(AppEvent_TransferFailed(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,error: null == error ? _self.error : error // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class AppEvent_WebShareStopped extends AppEvent {
  const AppEvent_WebShareStopped(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_WebShareStopped);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'AppEvent.webShareStopped()';
}


}




/// @nodoc


class AppEvent_OfferReceived extends AppEvent {
  const AppEvent_OfferReceived({required this.id, required this.deviceId, required this.fileName, required this.count, required this.size}): super._();
  

 final  PlatformInt64 id;
 final  String deviceId;
/// The first file's name.
 final  String fileName;
 final  PlatformInt64 count;
 final  PlatformInt64 size;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_OfferReceivedCopyWith<AppEvent_OfferReceived> get copyWith => _$AppEvent_OfferReceivedCopyWithImpl<AppEvent_OfferReceived>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_OfferReceived&&(identical(other.id, id) || other.id == id)&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.fileName, fileName) || other.fileName == fileName)&&(identical(other.count, count) || other.count == count)&&(identical(other.size, size) || other.size == size));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,deviceId,fileName,count,size);
}

@override
String toString() {
    return 'AppEvent.offerReceived(id: $id, deviceId: $deviceId, fileName: $fileName, count: $count, size: $size)';
}


}

/// @nodoc
abstract mixin class $AppEvent_OfferReceivedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_OfferReceivedCopyWith(AppEvent_OfferReceived value, $Res Function(AppEvent_OfferReceived) _then) = _$AppEvent_OfferReceivedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String deviceId, String fileName, PlatformInt64 count, PlatformInt64 size
});




}
/// @nodoc
class _$AppEvent_OfferReceivedCopyWithImpl<$Res>
    implements $AppEvent_OfferReceivedCopyWith<$Res> {
  _$AppEvent_OfferReceivedCopyWithImpl(this._self, this._then);

  final AppEvent_OfferReceived _self;
  final $Res Function(AppEvent_OfferReceived) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? deviceId = null,Object? fileName = null,Object? count = null,Object? size = null,}) {
  return _then(AppEvent_OfferReceived(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,fileName: null == fileName ? _self.fileName : fileName // ignore: cast_nullable_to_non_nullable
as String,count: null == count ? _self.count : count // ignore: cast_nullable_to_non_nullable
as PlatformInt64,size: null == size ? _self.size : size // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class AppEvent_OfferClosed extends AppEvent {
  const AppEvent_OfferClosed({required this.id}): super._();
  

 final  PlatformInt64 id;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_OfferClosedCopyWith<AppEvent_OfferClosed> get copyWith => _$AppEvent_OfferClosedCopyWithImpl<AppEvent_OfferClosed>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_OfferClosed&&(identical(other.id, id) || other.id == id));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id);
}

@override
String toString() {
    return 'AppEvent.offerClosed(id: $id)';
}


}

/// @nodoc
abstract mixin class $AppEvent_OfferClosedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_OfferClosedCopyWith(AppEvent_OfferClosed value, $Res Function(AppEvent_OfferClosed) _then) = _$AppEvent_OfferClosedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id
});




}
/// @nodoc
class _$AppEvent_OfferClosedCopyWithImpl<$Res>
    implements $AppEvent_OfferClosedCopyWith<$Res> {
  _$AppEvent_OfferClosedCopyWithImpl(this._self, this._then);

  final AppEvent_OfferClosed _self;
  final $Res Function(AppEvent_OfferClosed) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,}) {
  return _then(AppEvent_OfferClosed(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class AppEvent_TransferAccepted extends AppEvent {
  const AppEvent_TransferAccepted({required this.id}): super._();
  

 final  PlatformInt64 id;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_TransferAcceptedCopyWith<AppEvent_TransferAccepted> get copyWith => _$AppEvent_TransferAcceptedCopyWithImpl<AppEvent_TransferAccepted>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_TransferAccepted&&(identical(other.id, id) || other.id == id));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id);
}

@override
String toString() {
    return 'AppEvent.transferAccepted(id: $id)';
}


}

/// @nodoc
abstract mixin class $AppEvent_TransferAcceptedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_TransferAcceptedCopyWith(AppEvent_TransferAccepted value, $Res Function(AppEvent_TransferAccepted) _then) = _$AppEvent_TransferAcceptedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id
});




}
/// @nodoc
class _$AppEvent_TransferAcceptedCopyWithImpl<$Res>
    implements $AppEvent_TransferAcceptedCopyWith<$Res> {
  _$AppEvent_TransferAcceptedCopyWithImpl(this._self, this._then);

  final AppEvent_TransferAccepted _self;
  final $Res Function(AppEvent_TransferAccepted) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,}) {
  return _then(AppEvent_TransferAccepted(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class AppEvent_TransferPending extends AppEvent {
  const AppEvent_TransferPending({required this.id}): super._();
  

 final  PlatformInt64 id;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_TransferPendingCopyWith<AppEvent_TransferPending> get copyWith => _$AppEvent_TransferPendingCopyWithImpl<AppEvent_TransferPending>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_TransferPending&&(identical(other.id, id) || other.id == id));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id);
}

@override
String toString() {
    return 'AppEvent.transferPending(id: $id)';
}


}

/// @nodoc
abstract mixin class $AppEvent_TransferPendingCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_TransferPendingCopyWith(AppEvent_TransferPending value, $Res Function(AppEvent_TransferPending) _then) = _$AppEvent_TransferPendingCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id
});




}
/// @nodoc
class _$AppEvent_TransferPendingCopyWithImpl<$Res>
    implements $AppEvent_TransferPendingCopyWith<$Res> {
  _$AppEvent_TransferPendingCopyWithImpl(this._self, this._then);

  final AppEvent_TransferPending _self;
  final $Res Function(AppEvent_TransferPending) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,}) {
  return _then(AppEvent_TransferPending(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class AppEvent_TransferDelivered extends AppEvent {
  const AppEvent_TransferDelivered({required this.id}): super._();
  

 final  PlatformInt64 id;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_TransferDeliveredCopyWith<AppEvent_TransferDelivered> get copyWith => _$AppEvent_TransferDeliveredCopyWithImpl<AppEvent_TransferDelivered>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_TransferDelivered&&(identical(other.id, id) || other.id == id));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id);
}

@override
String toString() {
    return 'AppEvent.transferDelivered(id: $id)';
}


}

/// @nodoc
abstract mixin class $AppEvent_TransferDeliveredCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_TransferDeliveredCopyWith(AppEvent_TransferDelivered value, $Res Function(AppEvent_TransferDelivered) _then) = _$AppEvent_TransferDeliveredCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id
});




}
/// @nodoc
class _$AppEvent_TransferDeliveredCopyWithImpl<$Res>
    implements $AppEvent_TransferDeliveredCopyWith<$Res> {
  _$AppEvent_TransferDeliveredCopyWithImpl(this._self, this._then);

  final AppEvent_TransferDelivered _self;
  final $Res Function(AppEvent_TransferDelivered) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,}) {
  return _then(AppEvent_TransferDelivered(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

/// @nodoc


class AppEvent_TransferRoute extends AppEvent {
  const AppEvent_TransferRoute({required this.id, required this.route}): super._();
  

 final  PlatformInt64 id;
 final  String route;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_TransferRouteCopyWith<AppEvent_TransferRoute> get copyWith => _$AppEvent_TransferRouteCopyWithImpl<AppEvent_TransferRoute>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_TransferRoute&&(identical(other.id, id) || other.id == id)&&(identical(other.route, route) || other.route == route));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,route);
}

@override
String toString() {
    return 'AppEvent.transferRoute(id: $id, route: $route)';
}


}

/// @nodoc
abstract mixin class $AppEvent_TransferRouteCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_TransferRouteCopyWith(AppEvent_TransferRoute value, $Res Function(AppEvent_TransferRoute) _then) = _$AppEvent_TransferRouteCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String route
});




}
/// @nodoc
class _$AppEvent_TransferRouteCopyWithImpl<$Res>
    implements $AppEvent_TransferRouteCopyWith<$Res> {
  _$AppEvent_TransferRouteCopyWithImpl(this._self, this._then);

  final AppEvent_TransferRoute _self;
  final $Res Function(AppEvent_TransferRoute) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? route = null,}) {
  return _then(AppEvent_TransferRoute(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,route: null == route ? _self.route : route // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class AppEvent_ClipboardReceived extends AppEvent {
  const AppEvent_ClipboardReceived({required this.deviceId, required this.text}): super._();
  

 final  String deviceId;
 final  String text;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_ClipboardReceivedCopyWith<AppEvent_ClipboardReceived> get copyWith => _$AppEvent_ClipboardReceivedCopyWithImpl<AppEvent_ClipboardReceived>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_ClipboardReceived&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.text, text) || other.text == text));
}


@override
int get hashCode {
    return Object.hash(runtimeType,deviceId,text);
}

@override
String toString() {
    return 'AppEvent.clipboardReceived(deviceId: $deviceId, text: $text)';
}


}

/// @nodoc
abstract mixin class $AppEvent_ClipboardReceivedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_ClipboardReceivedCopyWith(AppEvent_ClipboardReceived value, $Res Function(AppEvent_ClipboardReceived) _then) = _$AppEvent_ClipboardReceivedCopyWithImpl;
@useResult
$Res call({
 String deviceId, String text
});




}
/// @nodoc
class _$AppEvent_ClipboardReceivedCopyWithImpl<$Res>
    implements $AppEvent_ClipboardReceivedCopyWith<$Res> {
  _$AppEvent_ClipboardReceivedCopyWithImpl(this._self, this._then);

  final AppEvent_ClipboardReceived _self;
  final $Res Function(AppEvent_ClipboardReceived) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,Object? text = null,}) {
  return _then(AppEvent_ClipboardReceived(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,text: null == text ? _self.text : text // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class AppEvent_ClipboardImageReceived extends AppEvent {
  const AppEvent_ClipboardImageReceived({required this.deviceId, required this.path, required this.mime}): super._();
  

 final  String deviceId;
 final  String path;
 final  String mime;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_ClipboardImageReceivedCopyWith<AppEvent_ClipboardImageReceived> get copyWith => _$AppEvent_ClipboardImageReceivedCopyWithImpl<AppEvent_ClipboardImageReceived>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_ClipboardImageReceived&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.path, path) || other.path == path)&&(identical(other.mime, mime) || other.mime == mime));
}


@override
int get hashCode {
    return Object.hash(runtimeType,deviceId,path,mime);
}

@override
String toString() {
    return 'AppEvent.clipboardImageReceived(deviceId: $deviceId, path: $path, mime: $mime)';
}


}

/// @nodoc
abstract mixin class $AppEvent_ClipboardImageReceivedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_ClipboardImageReceivedCopyWith(AppEvent_ClipboardImageReceived value, $Res Function(AppEvent_ClipboardImageReceived) _then) = _$AppEvent_ClipboardImageReceivedCopyWithImpl;
@useResult
$Res call({
 String deviceId, String path, String mime
});




}
/// @nodoc
class _$AppEvent_ClipboardImageReceivedCopyWithImpl<$Res>
    implements $AppEvent_ClipboardImageReceivedCopyWith<$Res> {
  _$AppEvent_ClipboardImageReceivedCopyWithImpl(this._self, this._then);

  final AppEvent_ClipboardImageReceived _self;
  final $Res Function(AppEvent_ClipboardImageReceived) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,Object? path = null,Object? mime = null,}) {
  return _then(AppEvent_ClipboardImageReceived(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,path: null == path ? _self.path : path // ignore: cast_nullable_to_non_nullable
as String,mime: null == mime ? _self.mime : mime // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class AppEvent_HotspotWanted extends AppEvent {
  const AppEvent_HotspotWanted({required this.id, required this.directOnly}): super._();
  

 final  PlatformInt64 id;
/// For screen mirroring: only a Wi-Fi Direct group, never a
/// local-only hotspot.
 final  bool directOnly;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_HotspotWantedCopyWith<AppEvent_HotspotWanted> get copyWith => _$AppEvent_HotspotWantedCopyWithImpl<AppEvent_HotspotWanted>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_HotspotWanted&&(identical(other.id, id) || other.id == id)&&(identical(other.directOnly, directOnly) || other.directOnly == directOnly));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,directOnly);
}

@override
String toString() {
    return 'AppEvent.hotspotWanted(id: $id, directOnly: $directOnly)';
}


}

/// @nodoc
abstract mixin class $AppEvent_HotspotWantedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_HotspotWantedCopyWith(AppEvent_HotspotWanted value, $Res Function(AppEvent_HotspotWanted) _then) = _$AppEvent_HotspotWantedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, bool directOnly
});




}
/// @nodoc
class _$AppEvent_HotspotWantedCopyWithImpl<$Res>
    implements $AppEvent_HotspotWantedCopyWith<$Res> {
  _$AppEvent_HotspotWantedCopyWithImpl(this._self, this._then);

  final AppEvent_HotspotWanted _self;
  final $Res Function(AppEvent_HotspotWanted) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? directOnly = null,}) {
  return _then(AppEvent_HotspotWanted(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,directOnly: null == directOnly ? _self.directOnly : directOnly // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class AppEvent_HotspotDone extends AppEvent {
  const AppEvent_HotspotDone(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_HotspotDone);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'AppEvent.hotspotDone()';
}


}




/// @nodoc


class AppEvent_JoinWanted extends AppEvent {
  const AppEvent_JoinWanted({required this.id, required this.ssid, required this.passphrase, required this.wpa3}): super._();
  

 final  PlatformInt64 id;
 final  String ssid;
 final  String passphrase;
 final  bool wpa3;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_JoinWantedCopyWith<AppEvent_JoinWanted> get copyWith => _$AppEvent_JoinWantedCopyWithImpl<AppEvent_JoinWanted>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_JoinWanted&&(identical(other.id, id) || other.id == id)&&(identical(other.ssid, ssid) || other.ssid == ssid)&&(identical(other.passphrase, passphrase) || other.passphrase == passphrase)&&(identical(other.wpa3, wpa3) || other.wpa3 == wpa3));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,ssid,passphrase,wpa3);
}

@override
String toString() {
    return 'AppEvent.joinWanted(id: $id, ssid: $ssid, passphrase: $passphrase, wpa3: $wpa3)';
}


}

/// @nodoc
abstract mixin class $AppEvent_JoinWantedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_JoinWantedCopyWith(AppEvent_JoinWanted value, $Res Function(AppEvent_JoinWanted) _then) = _$AppEvent_JoinWantedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String ssid, String passphrase, bool wpa3
});




}
/// @nodoc
class _$AppEvent_JoinWantedCopyWithImpl<$Res>
    implements $AppEvent_JoinWantedCopyWith<$Res> {
  _$AppEvent_JoinWantedCopyWithImpl(this._self, this._then);

  final AppEvent_JoinWanted _self;
  final $Res Function(AppEvent_JoinWanted) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? ssid = null,Object? passphrase = null,Object? wpa3 = null,}) {
  return _then(AppEvent_JoinWanted(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,ssid: null == ssid ? _self.ssid : ssid // ignore: cast_nullable_to_non_nullable
as String,passphrase: null == passphrase ? _self.passphrase : passphrase // ignore: cast_nullable_to_non_nullable
as String,wpa3: null == wpa3 ? _self.wpa3 : wpa3 // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class AppEvent_LeaveWanted extends AppEvent {
  const AppEvent_LeaveWanted(): super._();
  






@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_LeaveWanted);
}


@override
int get hashCode => runtimeType.hashCode;

@override
String toString() {
    return 'AppEvent.leaveWanted()';
}


}




/// @nodoc


class AppEvent_PairAsked extends AppEvent {
  const AppEvent_PairAsked({required this.id, required this.deviceId, required this.deviceName, required this.code}): super._();
  

 final  PlatformInt64 id;
 final  String deviceId;
 final  String deviceName;
 final  String code;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_PairAskedCopyWith<AppEvent_PairAsked> get copyWith => _$AppEvent_PairAskedCopyWithImpl<AppEvent_PairAsked>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_PairAsked&&(identical(other.id, id) || other.id == id)&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.deviceName, deviceName) || other.deviceName == deviceName)&&(identical(other.code, code) || other.code == code));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,deviceId,deviceName,code);
}

@override
String toString() {
    return 'AppEvent.pairAsked(id: $id, deviceId: $deviceId, deviceName: $deviceName, code: $code)';
}


}

/// @nodoc
abstract mixin class $AppEvent_PairAskedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_PairAskedCopyWith(AppEvent_PairAsked value, $Res Function(AppEvent_PairAsked) _then) = _$AppEvent_PairAskedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String deviceId, String deviceName, String code
});




}
/// @nodoc
class _$AppEvent_PairAskedCopyWithImpl<$Res>
    implements $AppEvent_PairAskedCopyWith<$Res> {
  _$AppEvent_PairAskedCopyWithImpl(this._self, this._then);

  final AppEvent_PairAsked _self;
  final $Res Function(AppEvent_PairAsked) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? deviceId = null,Object? deviceName = null,Object? code = null,}) {
  return _then(AppEvent_PairAsked(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,deviceName: null == deviceName ? _self.deviceName : deviceName // ignore: cast_nullable_to_non_nullable
as String,code: null == code ? _self.code : code // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class AppEvent_MirrorAsked extends AppEvent {
  const AppEvent_MirrorAsked({required this.id, required this.deviceId}): super._();
  

 final  PlatformInt64 id;
 final  String deviceId;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_MirrorAskedCopyWith<AppEvent_MirrorAsked> get copyWith => _$AppEvent_MirrorAskedCopyWithImpl<AppEvent_MirrorAsked>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_MirrorAsked&&(identical(other.id, id) || other.id == id)&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,deviceId);
}

@override
String toString() {
    return 'AppEvent.mirrorAsked(id: $id, deviceId: $deviceId)';
}


}

/// @nodoc
abstract mixin class $AppEvent_MirrorAskedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_MirrorAskedCopyWith(AppEvent_MirrorAsked value, $Res Function(AppEvent_MirrorAsked) _then) = _$AppEvent_MirrorAskedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String deviceId
});




}
/// @nodoc
class _$AppEvent_MirrorAskedCopyWithImpl<$Res>
    implements $AppEvent_MirrorAskedCopyWith<$Res> {
  _$AppEvent_MirrorAskedCopyWithImpl(this._self, this._then);

  final AppEvent_MirrorAsked _self;
  final $Res Function(AppEvent_MirrorAsked) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? deviceId = null,}) {
  return _then(AppEvent_MirrorAsked(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class AppEvent_MirrorStarted extends AppEvent {
  const AppEvent_MirrorStarted({required this.id, required this.deviceId, required this.sharing}): super._();
  

 final  PlatformInt64 id;
 final  String deviceId;
 final  bool sharing;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_MirrorStartedCopyWith<AppEvent_MirrorStarted> get copyWith => _$AppEvent_MirrorStartedCopyWithImpl<AppEvent_MirrorStarted>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_MirrorStarted&&(identical(other.id, id) || other.id == id)&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId)&&(identical(other.sharing, sharing) || other.sharing == sharing));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id,deviceId,sharing);
}

@override
String toString() {
    return 'AppEvent.mirrorStarted(id: $id, deviceId: $deviceId, sharing: $sharing)';
}


}

/// @nodoc
abstract mixin class $AppEvent_MirrorStartedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_MirrorStartedCopyWith(AppEvent_MirrorStarted value, $Res Function(AppEvent_MirrorStarted) _then) = _$AppEvent_MirrorStartedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id, String deviceId, bool sharing
});




}
/// @nodoc
class _$AppEvent_MirrorStartedCopyWithImpl<$Res>
    implements $AppEvent_MirrorStartedCopyWith<$Res> {
  _$AppEvent_MirrorStartedCopyWithImpl(this._self, this._then);

  final AppEvent_MirrorStarted _self;
  final $Res Function(AppEvent_MirrorStarted) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,Object? deviceId = null,Object? sharing = null,}) {
  return _then(AppEvent_MirrorStarted(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,sharing: null == sharing ? _self.sharing : sharing // ignore: cast_nullable_to_non_nullable
as bool,
  ));
}


}

/// @nodoc


class AppEvent_MirrorWaiting extends AppEvent {
  const AppEvent_MirrorWaiting({required this.deviceId}): super._();
  

 final  String deviceId;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_MirrorWaitingCopyWith<AppEvent_MirrorWaiting> get copyWith => _$AppEvent_MirrorWaitingCopyWithImpl<AppEvent_MirrorWaiting>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_MirrorWaiting&&(identical(other.deviceId, deviceId) || other.deviceId == deviceId));
}


@override
int get hashCode {
    return Object.hash(runtimeType,deviceId);
}

@override
String toString() {
    return 'AppEvent.mirrorWaiting(deviceId: $deviceId)';
}


}

/// @nodoc
abstract mixin class $AppEvent_MirrorWaitingCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_MirrorWaitingCopyWith(AppEvent_MirrorWaiting value, $Res Function(AppEvent_MirrorWaiting) _then) = _$AppEvent_MirrorWaitingCopyWithImpl;
@useResult
$Res call({
 String deviceId
});




}
/// @nodoc
class _$AppEvent_MirrorWaitingCopyWithImpl<$Res>
    implements $AppEvent_MirrorWaitingCopyWith<$Res> {
  _$AppEvent_MirrorWaitingCopyWithImpl(this._self, this._then);

  final AppEvent_MirrorWaiting _self;
  final $Res Function(AppEvent_MirrorWaiting) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? deviceId = null,}) {
  return _then(AppEvent_MirrorWaiting(
deviceId: null == deviceId ? _self.deviceId : deviceId // ignore: cast_nullable_to_non_nullable
as String,
  ));
}


}

/// @nodoc


class AppEvent_MirrorEnded extends AppEvent {
  const AppEvent_MirrorEnded({required this.id}): super._();
  

 final  PlatformInt64 id;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@JsonKey(includeFromJson: false, includeToJson: false)
@pragma('vm:prefer-inline')
$AppEvent_MirrorEndedCopyWith<AppEvent_MirrorEnded> get copyWith => _$AppEvent_MirrorEndedCopyWithImpl<AppEvent_MirrorEnded>(this, _$identity);



@override
bool operator ==(Object other) {
    return identical(this, other) || (other.runtimeType == runtimeType&&other is AppEvent_MirrorEnded&&(identical(other.id, id) || other.id == id));
}


@override
int get hashCode {
    return Object.hash(runtimeType,id);
}

@override
String toString() {
    return 'AppEvent.mirrorEnded(id: $id)';
}


}

/// @nodoc
abstract mixin class $AppEvent_MirrorEndedCopyWith<$Res> implements $AppEventCopyWith<$Res> {
  factory $AppEvent_MirrorEndedCopyWith(AppEvent_MirrorEnded value, $Res Function(AppEvent_MirrorEnded) _then) = _$AppEvent_MirrorEndedCopyWithImpl;
@useResult
$Res call({
 PlatformInt64 id
});




}
/// @nodoc
class _$AppEvent_MirrorEndedCopyWithImpl<$Res>
    implements $AppEvent_MirrorEndedCopyWith<$Res> {
  _$AppEvent_MirrorEndedCopyWithImpl(this._self, this._then);

  final AppEvent_MirrorEnded _self;
  final $Res Function(AppEvent_MirrorEnded) _then;

/// Create a copy of AppEvent
/// with the given fields replaced by the non-null parameter values.
@pragma('vm:prefer-inline') $Res call({Object? id = null,}) {
  return _then(AppEvent_MirrorEnded(
id: null == id ? _self.id : id // ignore: cast_nullable_to_non_nullable
as PlatformInt64,
  ));
}


}

// dart format on
