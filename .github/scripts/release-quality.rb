require 'json'
require 'open3'

# 仅复用同一源码的主分支 CI，缺少证据时完整检查发行提交
module ReleaseQuality
  def self.capture(*command)
    output, error, status = Open3.capture3(*command)
    raise "#{command.first} failed: #{error.strip}" unless status.success?
    output
  end

  def self.evidence(repository)
    parents = capture('git', 'rev-list', '--parents', '-n', '1', 'HEAD').split
    commits = [parents.first]
    # 优先核实当前提交；只有备版文件变化时才允许复用父提交的同一份源码。
    if parents.length == 2
      parent = parents.last
      changes = capture('git', 'diff', '--name-only', '-z', parent, 'HEAD').split("\0")
      commits << parent unless changes.empty? || (changes - %w[release/version.yaml release/notes.md]).any?
    end

    commits.each do |sha|
      # 只看最新一次运行；失败或仍在执行时不回退到旧的成功记录。
      endpoint = "repos/#{repository}/actions/workflows/ci.yml/runs?event=push&branch=main&head_sha=#{sha}&per_page=1"
      run = JSON.parse(capture('gh', 'api', endpoint)).fetch('workflow_runs').first
      next unless run
      return nil unless run.fetch('head_sha') == sha && run.fetch('conclusion') == 'success'
      return nil unless run.fetch('head_repository').fetch('full_name') == repository

      rows = capture('gh', 'api', '--paginate', "repos/#{repository}/actions/runs/#{run.fetch('id')}/jobs?per_page=100", '--jq', '.jobs[] | @json')
      job = rows.lines.map { |line| JSON.parse(line) }.find { |entry| entry.fetch('name') == 'quality / backend' }
      return job.fetch('html_url') if job && job.fetch('conclusion') == 'success'
      # 当前提交明确跳过后端检查时，才继续核实允许复用的父提交。
      return nil unless job && job.fetch('conclusion') == 'skipped'
    end
    nil
  end
end

if $PROGRAM_NAME == __FILE__
  begin
    evidence = ReleaseQuality.evidence(ENV.fetch('GITHUB_REPOSITORY'))
  rescue StandardError => error
    warn "CI evidence unavailable; running full release checks: #{error.message}"
    evidence = nil
  end
  puts "run_backend=#{evidence.nil?}"
  if ENV['GITHUB_STEP_SUMMARY']
    message = evidence ? "后端质量检查复用同一源码的主分支 [CI 结果](#{evidence})" : '未找到可复用的后端 CI 结果，完整检查发行提交'
    File.open(ENV.fetch('GITHUB_STEP_SUMMARY'), 'a') { |file| file.puts(message) }
  end
end
