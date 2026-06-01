import FluidAudio
import Foundation

struct DiarizationSegment: Encodable {
  let speakerId: String
  let startMs: Int64
  let endMs: Int64
}

struct DiarizationOutput: Encodable {
  let engine: String
  let model: String
  let generatedAt: String
  let sourceAudioPath: String
  let segments: [DiarizationSegment]
}

enum DiarizerCLIError: Error, CustomStringConvertible {
  case missingValue(String)
  case unknownArgument(String)
  case missingInput
  case missingOutput
  case inputMissing(String)

  var description: String {
    switch self {
    case .missingValue(let arg):
      return "missing value for \(arg)"
    case .unknownArgument(let arg):
      return "unknown argument \(arg)"
    case .missingInput:
      return "missing --input"
    case .missingOutput:
      return "missing --output"
    case .inputMissing(let path):
      return "input audio does not exist: \(path)"
    }
  }
}

struct CLIOptions {
  let input: String
  let output: String
}

@main
enum PohaDiarizer {
  static func main() async {
    do {
      let options = try parseOptions(Array(CommandLine.arguments.dropFirst()))
      try await diarize(options: options)
    } catch {
      fputs("poha-diarizer: \(error)\n", stderr)
      exit(1)
    }
  }

  private static func parseOptions(_ args: [String]) throws -> CLIOptions {
    var input: String?
    var output: String?
    var index = 0

    while index < args.count {
      let arg = args[index]
      switch arg {
      case "--input":
        index += 1
        guard index < args.count else { throw DiarizerCLIError.missingValue(arg) }
        input = args[index]
      case "--output":
        index += 1
        guard index < args.count else { throw DiarizerCLIError.missingValue(arg) }
        output = args[index]
      default:
        throw DiarizerCLIError.unknownArgument(arg)
      }
      index += 1
    }

    guard let input else { throw DiarizerCLIError.missingInput }
    guard let output else { throw DiarizerCLIError.missingOutput }
    guard FileManager.default.fileExists(atPath: input) else {
      throw DiarizerCLIError.inputMissing(input)
    }

    return CLIOptions(input: input, output: output)
  }

  private static func diarize(options: CLIOptions) async throws {
    let inputURL = URL(fileURLWithPath: options.input)
    let outputURL = URL(fileURLWithPath: options.output)
    try FileManager.default.createDirectory(
      at: outputURL.deletingLastPathComponent(),
      withIntermediateDirectories: true
    )

    let diarizer = DiarizerManager()
    let models = try await DiarizerModels.download()
    diarizer.initialize(models: models)

    let converter = AudioConverter()
    let samples = try converter.resampleAudioFile(inputURL)
    let result = try diarizer.performCompleteDiarization(samples, sampleRate: 16_000)

    let segments = result.segments.map { segment in
      DiarizationSegment(
        speakerId: segment.speakerId,
        startMs: Int64((segment.startTimeSeconds * 1_000).rounded()),
        endMs: Int64((segment.endTimeSeconds * 1_000).rounded())
      )
    }

    let payload = DiarizationOutput(
      engine: "FluidAudioCoreML",
      model: "speaker-diarization-coreml",
      generatedAt: ISO8601DateFormatter().string(from: Date()),
      sourceAudioPath: inputURL.path,
      segments: segments
    )

    let encoder = JSONEncoder()
    encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
    try encoder.encode(payload).write(to: outputURL, options: .atomic)
    fputs("poha-diarizer: wrote \(segments.count) segments to \(outputURL.path)\n", stderr)
  }
}
