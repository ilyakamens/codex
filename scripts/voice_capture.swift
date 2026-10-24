import AudioToolbox
import CoreAudio
import Foundation

func emit(_ value: [String: Any]) {
    do {
        let data = try JSONSerialization.data(withJSONObject: value)
        FileHandle.standardOutput.write(data + Data([10]))
    } catch {
        exit(1)
    }
}

func check(_ status: OSStatus) throws {
    if status != noErr {
        throw NSError(domain: NSOSStatusErrorDomain, code: Int(status))
    }
}

func microphoneID(named name: String) throws -> AudioDeviceID {
    var address = AudioObjectPropertyAddress(
        mSelector: kAudioHardwarePropertyDevices,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain
    )
    var size: UInt32 = 0
    try check(AudioObjectGetPropertyDataSize(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size))
    var devices = [AudioDeviceID](repeating: 0, count: Int(size) / MemoryLayout<AudioDeviceID>.size)
    try check(AudioObjectGetPropertyData(AudioObjectID(kAudioObjectSystemObject), &address, 0, nil, &size, &devices))
    for device in devices {
        var nameAddress = AudioObjectPropertyAddress(
            mSelector: kAudioObjectPropertyName,
            mScope: kAudioObjectPropertyScopeGlobal,
            mElement: kAudioObjectPropertyElementMain
        )
        var deviceName: Unmanaged<CFString>?
        var nameSize = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
        try check(AudioObjectGetPropertyData(device, &nameAddress, 0, nil, &nameSize, &deviceName))
        if let deviceName, deviceName.takeRetainedValue() as String == name { return device }
    }
    throw NSError(domain: "Dictation", code: 1, userInfo: [NSLocalizedDescriptionKey: "Microphone not found: \(name)"])
}

final class Capture {
    let lock = NSLock()
    var muted = false
    var ready = false
    var queue: AudioQueueRef?

    func setMuted(_ value: Bool) {
        lock.lock()
        muted = value
        lock.unlock()
    }

    func receive(_ buffer: AudioQueueBufferRef) {
        lock.lock()
        let isMuted = muted
        lock.unlock()
        let count = Int(buffer.pointee.mAudioDataByteSize) / MemoryLayout<Int16>.size
        let samples = buffer.pointee.mAudioData.assumingMemoryBound(to: Int16.self)
        var peak = 0
        var squares = 0.0
        for index in 0..<count {
            if isMuted { samples[index] = 0 }
            peak = max(peak, abs(Int(samples[index])))
            squares += Double(samples[index]) * Double(samples[index])
        }
        if !ready {
            ready = true
            emit(["type": "ready"])
        }
        let audio = Data(bytes: samples, count: count * MemoryLayout<Int16>.size)
        emit(["type": "audio", "audio": audio.base64EncodedString(), "peak": peak, "rms": Int(sqrt(squares / Double(count))), "samples": count])
    }

    func start() throws {
        var format = AudioStreamBasicDescription(
            mSampleRate: 16000,
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 2, mFramesPerPacket: 1,
            mBytesPerFrame: 2, mChannelsPerFrame: 1, mBitsPerChannel: 16, mReserved: 0
        )
        try check(AudioQueueNewInput(&format, { context, queue, buffer, _, _, _ in
            let capture = Unmanaged<Capture>.fromOpaque(context!).takeUnretainedValue()
            capture.receive(buffer)
            let status = AudioQueueEnqueueBuffer(queue, buffer, 0, nil)
            if status != noErr {
                emit(["type": "error", "message": "Microphone buffer failed: \(status)"])
                exit(1)
            }
        }, Unmanaged.passUnretained(self).toOpaque(), nil, nil, 0, &queue))
        guard let queue else {
            throw NSError(domain: "Dictation", code: 2, userInfo: [NSLocalizedDescriptionKey: "Could not create microphone recorder"])
        }
        if CommandLine.arguments.count > 1 {
            let device = try microphoneID(named: CommandLine.arguments[1])
            var address = AudioObjectPropertyAddress(
                mSelector: kAudioDevicePropertyDeviceUID,
                mScope: kAudioObjectPropertyScopeGlobal,
                mElement: kAudioObjectPropertyElementMain
            )
            var uid: Unmanaged<CFString>?
            var size = UInt32(MemoryLayout<Unmanaged<CFString>?>.size)
            try check(AudioObjectGetPropertyData(device, &address, 0, nil, &size, &uid))
            guard let uid else {
                throw NSError(domain: "Dictation", code: 3, userInfo: [NSLocalizedDescriptionKey: "Microphone has no device UID"])
            }
            defer { uid.release() }
            var reference: Unmanaged<CFString>? = uid
            try check(AudioQueueSetProperty(queue, kAudioQueueProperty_CurrentDevice, &reference, size))
        }
        for _ in 0..<3 {
            var buffer: AudioQueueBufferRef?
            try check(AudioQueueAllocateBuffer(queue, 3200, &buffer))
            if let buffer { try check(AudioQueueEnqueueBuffer(queue, buffer, 0, nil)) }
        }
        try check(AudioQueueStart(queue, nil))
    }
}

let capture = Capture()
do {
    try capture.start()
    DispatchQueue.global().async {
        while let line = readLine() {
            if line == "mute" { capture.setMuted(true) }
            else if line == "unmute" { capture.setMuted(false) }
            else if line == "stop" { exit(0) }
        }
        exit(0)
    }
    RunLoop.main.run()
} catch {
    emit(["type": "error", "message": error.localizedDescription])
    exit(1)
}
