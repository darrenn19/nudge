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

@optionalTypeArgs TResult maybeMap<TResult extends Object?>({TResult Function( AppEvent_DevicesChanged value)?  devicesChanged,TResult Function( AppEvent_Paired value)?  paired,TResult Function( AppEvent_PairingClosed value)?  pairingClosed,TResult Function( AppEvent_TransferStarted value)?  transferStarted,TResult Function( AppEvent_TransferProgress value)?  transferProgress,TResult Function( AppEvent_TransferDone value)?  transferDone,TResult Function( AppEvent_TransferFailed value)?  transferFailed,required TResult orElse(),}){
final _that = this;
switch (_that) {
case AppEvent_DevicesChanged() when devicesChanged != null:
return devicesChanged(_that);case AppEvent_Paired() when paired != null:
return paired(_that);case AppEvent_PairingClosed() when pairingClosed != null:
return pairingClosed(_that);case AppEvent_TransferStarted() when transferStarted != null:
return transferStarted(_that);case AppEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that);case AppEvent_TransferDone() when transferDone != null:
return transferDone(_that);case AppEvent_TransferFailed() when transferFailed != null:
return transferFailed(_that);case _:
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

@optionalTypeArgs TResult map<TResult extends Object?>({required TResult Function( AppEvent_DevicesChanged value)  devicesChanged,required TResult Function( AppEvent_Paired value)  paired,required TResult Function( AppEvent_PairingClosed value)  pairingClosed,required TResult Function( AppEvent_TransferStarted value)  transferStarted,required TResult Function( AppEvent_TransferProgress value)  transferProgress,required TResult Function( AppEvent_TransferDone value)  transferDone,required TResult Function( AppEvent_TransferFailed value)  transferFailed,}){
final _that = this;
switch (_that) {
case AppEvent_DevicesChanged():
return devicesChanged(_that);case AppEvent_Paired():
return paired(_that);case AppEvent_PairingClosed():
return pairingClosed(_that);case AppEvent_TransferStarted():
return transferStarted(_that);case AppEvent_TransferProgress():
return transferProgress(_that);case AppEvent_TransferDone():
return transferDone(_that);case AppEvent_TransferFailed():
return transferFailed(_that);}
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

@optionalTypeArgs TResult? mapOrNull<TResult extends Object?>({TResult? Function( AppEvent_DevicesChanged value)?  devicesChanged,TResult? Function( AppEvent_Paired value)?  paired,TResult? Function( AppEvent_PairingClosed value)?  pairingClosed,TResult? Function( AppEvent_TransferStarted value)?  transferStarted,TResult? Function( AppEvent_TransferProgress value)?  transferProgress,TResult? Function( AppEvent_TransferDone value)?  transferDone,TResult? Function( AppEvent_TransferFailed value)?  transferFailed,}){
final _that = this;
switch (_that) {
case AppEvent_DevicesChanged() when devicesChanged != null:
return devicesChanged(_that);case AppEvent_Paired() when paired != null:
return paired(_that);case AppEvent_PairingClosed() when pairingClosed != null:
return pairingClosed(_that);case AppEvent_TransferStarted() when transferStarted != null:
return transferStarted(_that);case AppEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that);case AppEvent_TransferDone() when transferDone != null:
return transferDone(_that);case AppEvent_TransferFailed() when transferFailed != null:
return transferFailed(_that);case _:
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

@optionalTypeArgs TResult maybeWhen<TResult extends Object?>({TResult Function()?  devicesChanged,TResult Function( DeviceInfo device)?  paired,TResult Function()?  pairingClosed,TResult Function( PlatformInt64 id,  String deviceId,  bool incoming,  String fileName,  PlatformInt64 size)?  transferStarted,TResult Function( PlatformInt64 id,  PlatformInt64 bytes,  PlatformInt64 size,  double bytesPerSec)?  transferProgress,TResult Function( PlatformInt64 id,  String path,  PlatformInt64 bytes,  double seconds,  PlatformInt64 resumedFrom)?  transferDone,TResult Function( PlatformInt64 id,  String error)?  transferFailed,required TResult orElse(),}) {final _that = this;
switch (_that) {
case AppEvent_DevicesChanged() when devicesChanged != null:
return devicesChanged();case AppEvent_Paired() when paired != null:
return paired(_that.device);case AppEvent_PairingClosed() when pairingClosed != null:
return pairingClosed();case AppEvent_TransferStarted() when transferStarted != null:
return transferStarted(_that.id,_that.deviceId,_that.incoming,_that.fileName,_that.size);case AppEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that.id,_that.bytes,_that.size,_that.bytesPerSec);case AppEvent_TransferDone() when transferDone != null:
return transferDone(_that.id,_that.path,_that.bytes,_that.seconds,_that.resumedFrom);case AppEvent_TransferFailed() when transferFailed != null:
return transferFailed(_that.id,_that.error);case _:
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

@optionalTypeArgs TResult when<TResult extends Object?>({required TResult Function()  devicesChanged,required TResult Function( DeviceInfo device)  paired,required TResult Function()  pairingClosed,required TResult Function( PlatformInt64 id,  String deviceId,  bool incoming,  String fileName,  PlatformInt64 size)  transferStarted,required TResult Function( PlatformInt64 id,  PlatformInt64 bytes,  PlatformInt64 size,  double bytesPerSec)  transferProgress,required TResult Function( PlatformInt64 id,  String path,  PlatformInt64 bytes,  double seconds,  PlatformInt64 resumedFrom)  transferDone,required TResult Function( PlatformInt64 id,  String error)  transferFailed,}) {final _that = this;
switch (_that) {
case AppEvent_DevicesChanged():
return devicesChanged();case AppEvent_Paired():
return paired(_that.device);case AppEvent_PairingClosed():
return pairingClosed();case AppEvent_TransferStarted():
return transferStarted(_that.id,_that.deviceId,_that.incoming,_that.fileName,_that.size);case AppEvent_TransferProgress():
return transferProgress(_that.id,_that.bytes,_that.size,_that.bytesPerSec);case AppEvent_TransferDone():
return transferDone(_that.id,_that.path,_that.bytes,_that.seconds,_that.resumedFrom);case AppEvent_TransferFailed():
return transferFailed(_that.id,_that.error);}
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

@optionalTypeArgs TResult? whenOrNull<TResult extends Object?>({TResult? Function()?  devicesChanged,TResult? Function( DeviceInfo device)?  paired,TResult? Function()?  pairingClosed,TResult? Function( PlatformInt64 id,  String deviceId,  bool incoming,  String fileName,  PlatformInt64 size)?  transferStarted,TResult? Function( PlatformInt64 id,  PlatformInt64 bytes,  PlatformInt64 size,  double bytesPerSec)?  transferProgress,TResult? Function( PlatformInt64 id,  String path,  PlatformInt64 bytes,  double seconds,  PlatformInt64 resumedFrom)?  transferDone,TResult? Function( PlatformInt64 id,  String error)?  transferFailed,}) {final _that = this;
switch (_that) {
case AppEvent_DevicesChanged() when devicesChanged != null:
return devicesChanged();case AppEvent_Paired() when paired != null:
return paired(_that.device);case AppEvent_PairingClosed() when pairingClosed != null:
return pairingClosed();case AppEvent_TransferStarted() when transferStarted != null:
return transferStarted(_that.id,_that.deviceId,_that.incoming,_that.fileName,_that.size);case AppEvent_TransferProgress() when transferProgress != null:
return transferProgress(_that.id,_that.bytes,_that.size,_that.bytesPerSec);case AppEvent_TransferDone() when transferDone != null:
return transferDone(_that.id,_that.path,_that.bytes,_that.seconds,_that.resumedFrom);case AppEvent_TransferFailed() when transferFailed != null:
return transferFailed(_that.id,_that.error);case _:
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

// dart format on
